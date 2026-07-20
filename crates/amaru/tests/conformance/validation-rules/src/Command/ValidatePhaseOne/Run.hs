{-# LANGUAGE DataKinds #-}

module Command.ValidatePhaseOne.Run
    ( run
    ) where

import Relude

import Cardano.Ledger.Api.Tx
    ( IsValid (..)
    , isValidTxL
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
    ( buildNewEpochState
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
        pure result
    when (any isLeft results) (liftIO exitFailure)

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
validateTestCase TestCase{network, eraHistory, protocolParameters, initialState, point, transaction, expected} = do
    globals <- buildGlobals network eraHistory point
    newEpochState <- buildNewEpochState (pparams protocolParameters) eraHistory initialState point

    let expectedPredicateHint = expectedPredicate expected
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
                            normalizeApplyTxError expectedPredicateHint err
                        Right _ ->
                            []

    case (expected, actualPredicates) of
        (ExpectedPass, []) ->
            Right "PASS"
        (ExpectedPass, predicates) ->
            Left (ValidationMismatch "PASS" (renderActualPredicates predicates))
        (ExpectedFailure predicateName, [actualPredicate])
            | predicateName == actualPredicate ->
                Right predicateName
        (ExpectedFailure predicateName, []) ->
            Left (ValidationMismatch predicateName "Pass")
        (ExpectedFailure predicateName, predicates) ->
            Left (ValidationMismatch predicateName (renderActualPredicates predicates))

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
    securityParameter <- inferSecurityParameter (FixtureEraHistory.stabilityWindow eraHistory) activeSlotCoeff
    epochInfo <- buildEpochInfo eraHistory point

    pure
        Globals
            { epochInfo = epochInfo
            , slotsPerKESPeriod = 129600
            , stabilityWindow = FixtureEraHistory.stabilityWindow eraHistory
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

normalizeApplyTxError :: Maybe Text -> ApplyTxError ConwayEra -> [Text]
normalizeApplyTxError expectedHint (ConwayApplyTxError failures) =
    map (normalizeLedgerFailure expectedHint) (NonEmpty.toList failures)

normalizeLedgerFailure :: Maybe Text -> ConwayLedgerPredFailure ConwayEra -> Text
normalizeLedgerFailure expectedHint = \case
    ConwayUtxowFailure failure ->
        normalizeUtxowFailure expectedHint failure
    ConwayCertsFailure failure ->
        normalizeCertsFailure expectedHint failure
    ConwayTxRefScriptsSizeTooBig{} ->
        "ConwayTxRefScriptsSizeTooBig"
    ConwayMempoolFailure message
        | "All inputs are spent." `Text.isPrefixOf` message ->
            "BadInputsUTxO"
    otherFailure ->
        "unsupported:" <> showText otherFailure

normalizeUtxowFailure :: Maybe Text -> ConwayUtxowPredFailure ConwayEra -> Text
normalizeUtxowFailure expectedHint = \case
    UtxoFailure failure ->
        normalizeUtxoFailure failure
    InvalidWitnessesUTXOW{} ->
        "InvalidWitnessesUTXOW"
    MissingVKeyWitnessesUTXOW{} ->
        "MissingVKeyWitnessesUTXOW"
    MissingTxBodyMetadataHash{} ->
        "MissingTxBodyMetadataHash"
    MissingTxMetadata{} ->
        "MissingTxMetadata"
    ConflictingMetadataHash{} ->
        "ConflictingMetadataHash"
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
    otherFailure ->
        "unsupported:" <> showText otherFailure

normalizeCertsFailure :: Maybe Text -> ConwayCertsPredFailure ConwayEra -> Text
normalizeCertsFailure expectedHint = \case
    CertFailure failure ->
        normalizeCertFailure expectedHint failure
    otherFailure ->
        "unsupported:" <> showText otherFailure

normalizeCertFailure :: Maybe Text -> ConwayCertPredFailure ConwayEra -> Text
normalizeCertFailure expectedHint = \case
    DelegFailure failure ->
        normalizeDelegFailure expectedHint failure
    GovCertFailure failure ->
        normalizeGovCertFailure failure
    otherFailure ->
        "unsupported:" <> showText otherFailure

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
    otherFailure ->
        "unsupported:" <> showText otherFailure

normalizeGovCertFailure :: ConwayGovCertPredFailure ConwayEra -> Text
normalizeGovCertFailure = \case
    ConwayDRepAlreadyRegistered{} ->
        "DRepAlreadyRegistered"
    otherFailure ->
        "unsupported:" <> showText otherFailure
