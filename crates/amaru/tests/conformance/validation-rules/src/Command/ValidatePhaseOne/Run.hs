{-# LANGUAGE DataKinds #-}

module Command.ValidatePhaseOne.Run
    ( run
    ) where

import Relude

import Cardano.Ledger.Api.Tx
    ( IsValid (..)
    , bodyTxL
    , isValidTxL
    )
import Cardano.Ledger.Api.Tx.Body
    ( inputsTxBodyL
    )
import Cardano.Ledger.BaseTypes
    ( ActiveSlotCoeff
    , BoundedRational (boundRational)
    , Globals (..)
    , mkActiveSlotCoeff
    , unsafeNonZero
    )
import Cardano.Ledger.Conway
    ( ApplyTxError (ConwayApplyTxError)
    , ConwayEra
    )
import Cardano.Ledger.Conway.Rules
    ( ConwayCertPredFailure (..)
    , ConwayCertsPredFailure (..)
    , ConwayDelegPredFailure (..)
    , ConwayGovCertPredFailure (..)
    , ConwayGovPredFailure (..)
    , ConwayLedgerPredFailure (..)
    , ConwayUtxoPredFailure (..)
    , ConwayUtxowPredFailure (..)
    )
import Cardano.Ledger.Conway.UTxO
    ( txNonDistinctRefScriptsSize
    )
import Cardano.Ledger.Core
    ( Tx
    , TxLevel (TopTx)
    )
import Cardano.Ledger.Shelley.API
    ( applyTx
    , mkMempoolEnv
    , mkMempoolState
    )
import Cardano.Ledger.Shelley.LedgerState
    ( EpochState (..)
    , LedgerState (..)
    , NewEpochState (..)
    , UTxOState (utxosUtxo)
    )
import Cardano.Ledger.Shelley.Rules
    ( ShelleyPoolPredFailure (..)
    )
import Cardano.Ledger.Shelley.StabilityWindow
    ( computeRandomnessStabilisationWindow
    , computeStabilityWindow
    )
import Cardano.Slotting.Time
    ( SystemStart (..)
    )
import Command.ValidatePhaseOne.Error
    ( Error (..)
    )
import Command.ValidatePhaseOne.Parse
    ( Options (..)
    , Target (..)
    )
import Data.Aeson
    ( Value
    , encode
    , object
    , (.=)
    )
import Data.Fixture.Common
    ( showText
    )
import Data.Fixture.EraHistory
    ( EraHistory
    , buildEpochInfo
    )
import qualified Data.Fixture.EraHistory as FixtureEraHistory
import Data.Fixture.Expected
    ( Expected (..)
    , expectedPredicate
    )
import Data.Fixture.InitialState
    ( InitialState
    , buildNewEpochState
    )
import Data.Fixture.Network
    ( FixtureNetwork
    , fixtureNetworkId
    )
import Data.Fixture.Point
    ( Point
    , pointSlotNo
    )
import Data.Fixture.ProtocolParameters
    ( ProtocolParameters (..)
    )
import Data.Fixture.TestCase
    ( TestCase (..)
    , loadTestCase
    , testCaseLabel
    )
import Data.Ratio
    ( (%)
    )
import Data.Time.Clock.POSIX
    ( posixSecondsToUTCTime
    )
import Lens.Micro
    ( (^.)
    )
import System.Directory
    ( doesDirectoryExist
    , listDirectory
    )
import System.FilePath
    ( takeExtension
    , (</>)
    )

import qualified Data.List.NonEmpty as NonEmpty
import qualified Data.Text as Text
import qualified Data.Text.IO as Text.IO

run :: Options -> ExceptT Error IO ()
run Options{target} =
    case target of
        SingleTestCase testCasePath ->
            runSingleTestCase testCasePath
        TestCaseDirectory directory ->
            runTestCaseDirectory directory

runSingleTestCase :: FilePath -> ExceptT Error IO ()
runSingleTestCase testCasePath = do
    (label, validationLabel) <- validateTestCaseAt testCasePath
    liftIO (putTextLn (label <> ": " <> validationLabel))

runTestCaseDirectory :: FilePath -> ExceptT Error IO ()
runTestCaseDirectory directory = do
    testCasePaths <- sort <$> liftIO (findTestCases directory)
    results <- forM testCasePaths $ \testCasePath -> liftIO $ do
        result <- runExceptT (validateTestCaseAt testCasePath)
        putLBSLn (encode (testCaseResult testCasePath result))
        pure (testCasePath, result)
    liftIO (Text.IO.hPutStr stderr (renderRunSummary directory results))
    when (any (isLeft . snd) results) (liftIO exitFailure)

renderRunSummary :: FilePath -> [(FilePath, Either Error (Text, Text))] -> Text
renderRunSummary directory results =
    Text.unlines (["", top, titleLine, separator] <> map renderRow rows <> [bottom])
  where
    total = length results
    failed = length [() | (_, Left _) <- results]
    passed = total - failed

    title = "Validation rules conformance summary"
    rows =
        [ ("  ", "directory", toText directory)
        , ("  ", "total", showText total)
        , (green "✓ ", "passed", showText passed)
        , (red "✗ ", "failed", showText failed)
        ]

    keyWidth = foldl' max 0 (map (\(_, key, _) -> Text.length key) rows)
    plainRow (_, key, value) = "  " <> Text.justifyLeft keyWidth ' ' key <> " : " <> value
    coloredRow (marker, key, value) = marker <> Text.justifyLeft keyWidth ' ' key <> " : " <> value
    innerWidth = foldl' max (Text.length title) (map (Text.length . plainRow) rows)

    horizontal = Text.replicate (innerWidth + 2) "─"
    top = "╭" <> horizontal <> "╮"
    separator = "├" <> horizontal <> "┤"
    bottom = "╰" <> horizontal <> "╯"
    titleLine = "│ " <> Text.justifyLeft innerWidth ' ' title <> " │"
    renderRow row =
        "│ " <> coloredRow row <> Text.replicate (innerWidth - Text.length (plainRow row)) " " <> " │"

    green text = "\ESC[32m" <> text <> "\ESC[0m"
    red text = "\ESC[31m" <> text <> "\ESC[0m"

validateTestCaseAt :: FilePath -> ExceptT Error IO (Text, Text)
validateTestCaseAt testCasePath = do
    testCase <- loadTestCase testCasePath
    validationLabel <- hoistEither (first (NamedError (testCaseLabel testCase)) (validateTestCase testCase))
    pure (testCaseLabel testCase, validationLabel)

testCaseResult :: FilePath -> Either Error (Text, Text) -> Value
testCaseResult testCasePath = \case
    Right (label, validationLabel) ->
        object
            [ "path" .= testCasePath
            , "label" .= label
            , "result" .= validationLabel
            ]
    Left validationError ->
        object
            [ "path" .= testCasePath
            , "error" .= validationError
            ]

findTestCases :: FilePath -> IO [FilePath]
findTestCases directory = do
    entries <- listDirectory directory
    fmap concat $ forM entries $ \entry -> do
        let path = directory </> entry
        isDirectory <- doesDirectoryExist path
        if isDirectory
            then findTestCases path
            else pure [path | takeExtension path == ".json"]

validateTestCase :: TestCase -> Either Error Text
validateTestCase TestCase{network, eraHistory, protocolParameters, initialState, point, transaction, expected} =
    case (expected, transaction) of
        (ExpectedDecodingFailure, Left _) ->
            Right "DecodingFailure"
        (ExpectedDecodingFailure, Right _) ->
            Left (ValidationMismatch "DecodingFailure" "decoded successfully")
        (_, Left decodeError) ->
            Left decodeError
        (_, Right decodedTransaction) ->
            validateDecodedTransaction network eraHistory protocolParameters initialState point decodedTransaction expected

validateDecodedTransaction
    :: FixtureNetwork
    -> EraHistory
    -> ProtocolParameters
    -> InitialState
    -> Point
    -> Tx TopTx ConwayEra
    -> Expected
    -> Either Error Text
validateDecodedTransaction network eraHistory protocolParameters initialState point transaction expected = do
    globals <- buildGlobals network eraHistory point
    newEpochState <- buildNewEpochState (pparams protocolParameters) eraHistory initialState point

    let expectedPredicateHint = expectedPredicate expected
    let emptyInputs = null (transaction ^. bodyTxL . inputsTxBodyL)
    let utxoState = esLState (nesEs newEpochState)
    let actualPredicates =
            case manualRefScriptSizeFailure protocolParameters utxoState transaction of
                Just predicateName ->
                    [predicateName]
                Nothing ->
                    case
                        applyTx
                            globals
                            (mkMempoolEnv newEpochState (pointSlotNo point))
                            (mkMempoolState newEpochState)
                            transaction
                     of
                        Left err ->
                            normalizeApplyTxError emptyInputs expectedPredicateHint err
                        Right _ ->
                            []

    case (expected, actualPredicates) of
        (ExpectedPass, []) ->
            Right "PASS"
        (ExpectedPass, predicates) ->
            Left (ValidationMismatch "PASS" (renderActualPredicates predicates))
        (ExpectedFailure predicateName, predicates)
            | predicateName `elem` predicates ->
                Right predicateName
            | null predicates ->
                Left (ValidationMismatch predicateName "PASS")
            | otherwise ->
                Left (ValidationMismatch predicateName (renderActualPredicates predicates))
        (ExpectedDecodingFailure, _) ->
            Left (ValidationMismatch "DecodingFailure" "decoded successfully")

manualRefScriptSizeFailure
    :: ProtocolParameters
    -> LedgerState ConwayEra
    -> Tx TopTx ConwayEra
    -> Maybe Text
manualRefScriptSizeFailure ProtocolParameters{maxReferenceScriptsSize} (LedgerState utxoState _) tx
    | tx ^. isValidTxL /= IsValid True =
        Nothing
    | totalReferenceScriptSize > maxReferenceScriptLimit =
        Just "ConwayTxRefScriptsSizeTooBig"
    | otherwise =
        Nothing
  where
    totalReferenceScriptSize =
        txNonDistinctRefScriptsSize (utxosUtxo utxoState) tx
    maxReferenceScriptLimit =
        fromIntegral maxReferenceScriptsSize

buildGlobals :: FixtureNetwork -> EraHistory -> Point -> Either Error Globals
buildGlobals network eraHistory point = do
    let activeSlotCoeff = defaultActiveSlotCoeff
    securityParameter <- inferSecurityParameter (FixtureEraHistory.stability_window eraHistory) activeSlotCoeff
    epochInfo <- buildEpochInfo eraHistory point

    pure
        Globals
            { epochInfo = epochInfo
            , slotsPerKESPeriod = 129600
            , stabilityWindow = FixtureEraHistory.stability_window eraHistory
            , randomnessStabilisationWindow =
                computeRandomnessStabilisationWindow securityParameter activeSlotCoeff
            , securityParameter = unsafeNonZero securityParameter
            , maxKESEvo = 62
            , quorum = 5
            , maxLovelaceSupply = 45000000000000000
            , activeSlotCoeff = activeSlotCoeff
            , networkId = fixtureNetworkId network
            , systemStart = SystemStart (posixSecondsToUTCTime 0)
            }

defaultActiveSlotCoeff :: ActiveSlotCoeff
defaultActiveSlotCoeff =
    case boundRational (1 % 20) of
        Just value ->
            mkActiveSlotCoeff value
        Nothing ->
            error "Invalid ActiveSlotCoeff placeholder in Command.ValidatePhaseOne.Run"

inferSecurityParameter :: Word64 -> ActiveSlotCoeff -> Either Error Word64
inferSecurityParameter stabilityWindowValue activeSlotCoeff =
    maybe
        (Left (UnsupportedFixture ("could not infer the security parameter from stabilityWindow=" <> showText stabilityWindowValue)))
        Right
        (find (\candidate -> computeStabilityWindow candidate activeSlotCoeff == stabilityWindowValue) [1 .. stabilityWindowValue])

renderActualPredicates :: [Text] -> Text
renderActualPredicates predicates =
    "[" <> Text.intercalate ", " predicates <> "]"

normalizeApplyTxError :: Bool -> Maybe Text -> ApplyTxError ConwayEra -> [Text]
normalizeApplyTxError emptyInputs expectedHint (ConwayApplyTxError failures) =
    map (normalizeLedgerFailure emptyInputs expectedHint) (filter isBlockValidationFailure (NonEmpty.toList failures))

-- Amaru's phase-one rules validate a transaction as part of a block, so failures 'applyTx' raises
-- that block application would not are dropped.
isBlockValidationFailure :: ConwayLedgerPredFailure ConwayEra -> Bool
isBlockValidationFailure =
    not . isMempoolOnlyFailure

-- 'applyTx' additionally runs the MEMPOOL rule, which only gates mempool admission. Below
-- protocol version 11 it stands in for the GOV rule's 'UnelectedCommitteeVoters' predicate, so a
-- vote by an unelected committee member is refused from the mempool while a block carrying it is
-- still accepted. From protocol version 11 the GOV predicate takes over and is reported normally.
isMempoolOnlyFailure :: ConwayLedgerPredFailure ConwayEra -> Bool
isMempoolOnlyFailure = \case
    ConwayMempoolFailure message ->
        "Unelected committee members are not allowed to cast votes" `Text.isPrefixOf` message
    _ ->
        False

normalizeLedgerFailure :: Bool -> Maybe Text -> ConwayLedgerPredFailure ConwayEra -> Text
normalizeLedgerFailure emptyInputs expectedHint = \case
    ConwayUtxowFailure failure ->
        normalizeUtxowFailure expectedHint failure
    ConwayCertsFailure failure ->
        normalizeCertsFailure expectedHint failure
    ConwayGovFailure failure ->
        normalizeGovFailure failure
    ConwayTxRefScriptsSizeTooBig{} ->
        "ConwayTxRefScriptsSizeTooBig"
    ConwayWdrlNotDelegatedToDRep{} ->
        "ConwayWdrlNotDelegatedToDRep"
    ConwayTreasuryValueMismatch{} ->
        "ConwayTreasuryValueMismatch"
    ConwayMempoolFailure message
        | "All inputs are spent." `Text.isPrefixOf` message ->
            if emptyInputs then "InputSetEmptyUTxO" else "BadInputsUTxO"
    otherFailure ->
        "unsupported:" <> showText otherFailure

normalizeUtxowFailure :: Maybe Text -> ConwayUtxowPredFailure ConwayEra -> Text
normalizeUtxowFailure expectedHint = \case
    UtxoFailure failure ->
        normalizeUtxoFailure failure
    InvalidWitnessesUTXOW{} ->
        "InvalidWitnessesUTXOW"
    MissingVKeyWitnessesUTXOW{} ->
        "MissingVerificationKeyWitnessesUTXOW"
    MissingTxBodyMetadataHash{} ->
        "MissingTxBodyMetadataHash"
    MissingTxMetadata{} ->
        "MissingTxMetadata"
    ConflictingMetadataHash{} ->
        "ConflictingMetadataHash"
    MalformedReferenceScripts{} ->
        "MalformedReferenceScripts"
    otherFailure ->
        "unsupported:" <> showText expectedHint <> ":" <> showText otherFailure

normalizeUtxoFailure :: ConwayUtxoPredFailure ConwayEra -> Text
normalizeUtxoFailure = \case
    BadInputsUTxO{} ->
        "BadInputsUTxO"
    OutsideValidityIntervalUTxO{} ->
        "OutsideValidityIntervalUTxO"
    MaxTxSizeUTxO{} ->
        "MaxTxSizeUTxO"
    InputSetEmptyUTxO ->
        "InputSetEmptyUTxO"
    FeeTooSmallUTxO{} ->
        "FeeTooSmallUTxO"
    ValueNotConservedUTxO{} ->
        "ValueNotConservedUTxO"
    WrongNetwork{} ->
        "WrongNetworkInTxOutput"
    WrongNetworkWithdrawal{} ->
        "WrongNetworkWithdrawal"
    OutputTooBigUTxO{} ->
        "OutputTooBigUTxO"
    InsufficientCollateral{} ->
        "InsufficientCollateral"
    IncorrectTotalCollateralField{} ->
        "IncorrectTotalCollateralField"
    TooManyCollateralInputs{} ->
        "TooManyCollateralInputs"
    ScriptsNotPaidUTxO{} ->
        "ScriptsNotPaidUTxO"
    WrongNetworkInTxBody{} ->
        "WrongNetworkInTxBody"
    OutsideForecast{} ->
        "OutsideForecast"
    OutputTooSmallUTxO{} ->
        "BabbageOutputTooSmallUTxO"
    BabbageOutputTooSmallUTxO{} ->
        "BabbageOutputTooSmallUTxO"
    BabbageNonDisjointRefInputs{} ->
        "BabbageNonDisjointRefInputs"
    UtxosFailure failure
        | "TimeTranslationPastHorizon" `Text.isInfixOf` showText failure ->
            "OutsideForecast"
        | "ValidationTagMismatch" `Text.isInfixOf` showText failure ->
            "ValidationTagMismatch"
    otherFailure ->
        "unsupported:" <> showText otherFailure

normalizeCertsFailure :: Maybe Text -> ConwayCertsPredFailure ConwayEra -> Text
normalizeCertsFailure expectedHint = \case
    CertFailure failure ->
        normalizeCertFailure expectedHint failure
    WithdrawalsNotInRewardsCERTS{} ->
        "WithdrawalsNotInRewardsCERTS"

normalizeCertFailure :: Maybe Text -> ConwayCertPredFailure ConwayEra -> Text
normalizeCertFailure expectedHint = \case
    DelegFailure failure ->
        normalizeDelegFailure expectedHint failure
    GovCertFailure failure ->
        normalizeGovCertFailure failure
    PoolFailure failure ->
        normalizePoolFailure failure

normalizeDelegFailure :: Maybe Text -> ConwayDelegPredFailure ConwayEra -> Text
normalizeDelegFailure expectedHint = \case
    StakeKeyRegisteredDELEG{} ->
        "StakeKeyRegistered"
    StakeKeyNotRegisteredDELEG{} ->
        fromMaybe "StakeCredentialInvalidPoolDelegation" expectedHint
    DelegateeDRepNotRegisteredDELEG{} ->
        "DelegateeDRepNotRegistered"
    DelegateeStakePoolNotRegisteredDELEG{} ->
        "DelegateeStakePoolNotRegistered"
    IncorrectDepositDELEG{} ->
        "IncorrectDepositDELEG"
    StakeKeyHasNonZeroAccountBalanceDELEG{} ->
        "StakeKeyHasNonZeroAccountBalance"
    otherFailure ->
        "unsupported:" <> showText otherFailure

normalizeGovCertFailure :: ConwayGovCertPredFailure ConwayEra -> Text
normalizeGovCertFailure = \case
    ConwayDRepAlreadyRegistered{} ->
        "DRepAlreadyRegistered"
    ConwayCommitteeIsUnknown{} ->
        "CommitteeIsUnknown"
    ConwayCommitteeHasPreviouslyResigned{} ->
        "CommitteeHasPreviouslyResigned"
    otherFailure ->
        "unsupported:" <> showText otherFailure

normalizePoolFailure :: ShelleyPoolPredFailure ConwayEra -> Text
normalizePoolFailure = \case
    StakePoolNotRegisteredOnKeyPOOL{} ->
        "StakePoolNotRegisteredOnKeyPOOL"
    StakePoolRetirementWrongEpochPOOL{} ->
        "StakePoolRetirementWrongEpochPOOL"
    StakePoolCostTooLowPOOL{} ->
        "StakePoolCostTooLowPOOL"
    otherFailure ->
        "unsupported:" <> showText otherFailure

normalizeGovFailure :: ConwayGovPredFailure ConwayEra -> Text
normalizeGovFailure = \case
    ProposalReturnAccountDoesNotExist{} ->
        "ProposalReturnAccountDoesNotExist"
    TreasuryWithdrawalReturnAccountsDoNotExist{} ->
        "TreasuryWithdrawalReturnAccountsDoNotExist"
    InvalidPrevGovActionId{} ->
        "InvalidPrevGovActionId"
    ZeroTreasuryWithdrawals{} ->
        "TreasuryWithdrawalsAllZeros"
    VotersDoNotExist{} ->
        "VotersDoNotExist"
    GovActionsDoNotExist{} ->
        "GovActionsDoNotExist"
    InvalidGuardrailsScriptHash{} ->
        "InvalidGuardrailsScriptHash"
    VotingOnExpiredGovAction{} ->
        "VotingOnExpiredGovAction"
    DisallowedVoters{} ->
        "DisallowedVoters"
    ProposalCantFollow{} ->
        "ProposalCantFollow"
    UnelectedCommitteeVoters{} ->
        "VotersDoNotExist"
    otherFailure ->
        "unsupported:" <> showText otherFailure
