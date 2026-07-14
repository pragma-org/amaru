{-# LANGUAGE DataKinds #-}
{-# LANGUAGE DerivingStrategies #-}
{-# LANGUAGE DeriveGeneric #-}
{-# LANGUAGE DuplicateRecordFields #-}
{-# LANGUAGE LambdaCase #-}
{-# LANGUAGE NamedFieldPuns #-}
{-# LANGUAGE PatternSynonyms #-}
{-# LANGUAGE RecordWildCards #-}
{-# LANGUAGE ScopedTypeVariables #-}
{-# LANGUAGE TypeApplications #-}

module Command.ValidatePhaseOne.Run
    ( run
    ) where

import Relude

import Cardano.Crypto.Hash
    ( hashFromTextAsHex
    )
import Cardano.Ledger.Api.Governance
    ( curPParamsGovStateL
    , emptyGovState
    , prevPParamsGovStateL
    )
import Cardano.Ledger.Api.PParams
    ( CoinPerByte (..)
    , PParams
    , emptyPParams
    , ppA0L
    , ppCoinsPerUTxOByteL
    , ppCollateralPercentageL
    , ppCostModelsL
    , ppEMaxL
    , ppKeyDepositL
    , ppMaxBBSizeL
    , ppMaxBHSizeL
    , ppMaxBlockExUnitsL
    , ppMaxCollateralInputsL
    , ppMaxTxExUnitsL
    , ppMaxTxSizeL
    , ppMaxValSizeL
    , ppMinPoolCostL
    , ppNOptL
    , ppPoolDepositL
    , ppPricesL
    , ppProtocolVersionL
    , ppRhoL
    , ppTauL
    , ppTxFeeFixedL
    , ppTxFeePerByteL
    )
import Cardano.Ledger.BaseTypes
    ( ActiveSlotCoeff
    , BoundedRational (boundRational)
    , EpochInterval (..)
    , Globals (..)
    , Network (Mainnet, Testnet)
    , NonNegativeInterval
    , ProtVer (..)
    , StrictMaybe (SNothing)
    , UnitInterval
    , integralToBounded
    , mkActiveSlotCoeff
    , unsafeNonZero
    )
import Cardano.Ledger.Binary
    ( DecCBOR (decCBOR)
    , decodeFull'
    , decodeFullAnnotator
    )
import Cardano.Ledger.Binary.Version
    ( Version
    , mkVersion
    )
import Cardano.Ledger.Binary.Plain
    ( withHexText
    )
import Cardano.Ledger.Coin
    ( Coin (..)
    )
import Cardano.Ledger.Compactible
    ( Compactible (..)
    )
import Cardano.Ledger.Conway
    ( ApplyTxError (ConwayApplyTxError)
    , ConwayEra
    )
import Cardano.Ledger.Conway.PParams
    ( DRepVotingThresholds (..)
    , PoolVotingThresholds (..)
    , ppCommitteeMaxTermLengthL
    , ppCommitteeMinSizeL
    , ppDRepActivityL
    , ppDRepDepositL
    , ppDRepVotingThresholdsL
    , ppGovActionDepositL
    , ppGovActionLifetimeL
    , ppMinFeeRefScriptCostPerByteL
    , ppPoolVotingThresholdsL
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
import Cardano.Ledger.Conway.State
    ( ConwayAccountState (..)
    , ConwayAccounts (..)
    , ConwayCertState (..)
    , VState (..)
    )
import Cardano.Ledger.Conway.UTxO
    ( txNonDistinctRefScriptsSize
    )
import Cardano.Ledger.Credential
    ( Credential
    )
import Cardano.Ledger.Api.Tx
    ( IsValid (..)
    , isValidTxL
    )
import Cardano.Ledger.Core
    ( Tx
    , TxLevel (TopTx)
    , TxOut
    , eraProtVerLow
    )
import Cardano.Ledger.DRep
    ( DRep (..)
    , DRepState (..)
    )
import Cardano.Ledger.Hashes
    ( GenDelegs (GenDelegs)
    , KeyHash (..)
    )
import Cardano.Ledger.Keys
    ( KeyRole (Staking)
    )
import Cardano.Ledger.Plutus.CostModels
    ( CostModels
    )
import Cardano.Ledger.Plutus.ExUnits
    ( ExUnits (..)
    , Prices (..)
    )
import qualified Cardano.Ledger.Slot as LedgerSlot
import Cardano.Ledger.Shelley.API
    ( applyTx
    , mkMempoolEnv
    , mkMempoolState
    )
import Cardano.Ledger.Shelley.LedgerState
    ( EpochState (..)
    , LedgerState (..)
    , NewEpochState (..)
    , UTxO (..)
    , UTxOState (..)
    )
import Cardano.Ledger.Shelley.StabilityWindow
    ( computeRandomnessStabilisationWindow
    , computeStabilityWindow
    )
import Cardano.Ledger.State
    ( DState (..)
    , PState (..)
    , ChainAccountState (..)
    , StakePoolState
    , spsDepositL
    )
import Cardano.Ledger.TxIn
    ( TxIn
    )
import Cardano.Slotting.EpochInfo
    ( hoistEpochInfo
    )
import Cardano.Slotting.Time
    ( RelativeTime (RelativeTime)
    , SystemStart (..)
    )
import Command.ValidatePhaseOne.Error
    ( Error (..)
    )
import Command.ValidatePhaseOne.Parse
    ( Options (..)
    )
import Control.Monad.Except
    ( runExcept
    )
import Data.Aeson
    ( FromJSON (..)
    , Object
    , Value (..)
    , eitherDecodeFileStrict'
    , withObject
    , withText
    , (.:)
    , (.:?)
    , (.!=)
    )
import qualified Data.Aeson.KeyMap as KeyMap
import Data.Aeson.Types
    ( Parser
    , parseEither
    )
import qualified Data.ByteString.Lazy as LazyByteString
import Data.Char
    ( isHexDigit
    )
import Data.Default.Class
    ( Default (def)
    )
import qualified Data.List.NonEmpty as NonEmpty
import qualified Data.Map.Strict as Map
import Data.Ratio
    ( (%)
    )
import qualified Data.SOP.NonEmpty as SOPNonEmpty
import qualified Data.Set as Set
import qualified Data.Text as Text
import Data.Time.Clock.POSIX
    ( posixSecondsToUTCTime
    )
import Lens.Micro
    ( (.~)
    , (^.)
    )
import qualified Cardano.Slotting.EpochInfo as EpochInfo
import Ouroboros.Consensus.Block
    ( EpochNo (EpochNo)
    , EpochSize (EpochSize)
    , SlotNo (SlotNo)
    )
import Ouroboros.Consensus.Block.Abstract
    ( GenesisWindow (GenesisWindow)
    )
import Ouroboros.Consensus.BlockchainTime.WallClock.Types
    ( slotLengthFromMillisec
    )
import Ouroboros.Consensus.HardFork.History.EpochInfo
    ( summaryToEpochInfo
    )
import Ouroboros.Consensus.HardFork.History.EraParams
    ( EraParams (..)
    , PerasEnabled
    , pattern NoPerasEnabled
    , SafeZone (UnsafeIndefiniteSafeZone)
    )
import Ouroboros.Consensus.HardFork.History.Summary
    ( Bound (..)
    , EraEnd (EraEnd)
    , EraSummary (..)
    , Summary (..)
    , mkUpperBound
    )
import System.FilePath
    ( takeDirectory
    , takeFileName
    , (</>)
    )

run :: Options -> ExceptT Error IO ()
run Options{fixturePath} = do
    rawFixture <- ExceptT (first (FixtureReadError fixturePath . toText) <$> liftIO (eitherDecodeFileStrict' fixturePath))
    fixtureRoot <- hoistEither (findFixtureRoot fixturePath)
    resolvedFixtureValue <- resolveFixtureValue fixtureRoot rawFixture
    resolvedFixture <- hoistEither (first (FixtureDecodeError fixturePath . toText) (parseEither parseJSON resolvedFixtureValue))
    validationLabel <- hoistEither (validateFixture resolvedFixture)
    liftIO (putTextLn ("Validated fixture: " <> validationLabel))

findFixtureRoot :: FilePath -> Either Error FilePath
findFixtureRoot fixturePath =
    go (takeDirectory fixturePath)
  where
    go directory
        | takeFileName directory == "phase-one" =
            Right directory
        | parent == directory =
            Left (FixtureReferenceError ("could not locate the phase-one fixture root from " <> toText fixturePath))
        | otherwise =
            go parent
      where
        parent = takeDirectory directory

resolveFixtureValue :: FilePath -> Value -> ExceptT Error IO Value
resolveFixtureValue fixtureRoot = \case
    Object objectValue -> do
        eraHistoryValue <-
            hoistEither (requireObjectKey "eraHistory" objectValue)
                >>= resolveSharedValue fixtureRoot "eraHistory"
        protocolParametersValue <-
            hoistEither (requireObjectKey "protocolParameters" objectValue)
                >>= resolveSharedValue fixtureRoot "protocolParameters"
        pure $
            Object
                ( KeyMap.insert
                    "protocolParameters"
                    protocolParametersValue
                    (KeyMap.insert "eraHistory" eraHistoryValue objectValue)
                )
    _ ->
        hoistEither (Left (FixtureDecodeError "" "expected a JSON object at the root"))

requireObjectKey :: Text -> Object -> Either Error Value
requireObjectKey key objectValue =
    maybe
        (Left (FixtureDecodeError "" ("missing required field \"" <> key <> "\"")))
        Right
        (KeyMap.lookup (fromString (toString key)) objectValue)

resolveSharedValue :: FilePath -> Text -> Value -> ExceptT Error IO Value
resolveSharedValue fixtureRoot fieldName = \case
    Object objectValue
        | Just (String relativePath) <- KeyMap.lookup "$ref" objectValue -> do
            referencedValue <- ExceptT $
                first (FixtureReadError (fixtureRoot </> toString relativePath) . toText)
                    <$> liftIO (eitherDecodeFileStrict' (fixtureRoot </> toString relativePath))
            mergedValue <- case KeyMap.lookup "$override" objectValue of
                Nothing ->
                    pure referencedValue
                Just (Object overrideValue) ->
                    case referencedValue of
                        Object referencedObject ->
                            pure (Object (KeyMap.union overrideValue referencedObject))
                        _ ->
                            hoistEither
                                ( Left
                                    ( FixtureReferenceError
                                        ( "field \""
                                            <> fieldName
                                            <> "\" uses $override, but the referenced value is not an object"
                                        )
                                    )
                                )
                Just _ ->
                    hoistEither
                        ( Left
                            ( FixtureReferenceError
                                ("field \"" <> fieldName <> "\" has a non-object $override")
                            )
                        )
            pure mergedValue
    inlineValue ->
        pure inlineValue

validateFixture :: ResolvedFixture -> Either Error Text
validateFixture ResolvedFixture{network, eraHistory, protocolParameters, initialState, point, transaction, expected} = do
    globals <- buildGlobals network eraHistory point
    pparams <- buildProtocolParameters protocolParameters
    newEpochState <- buildNewEpochState pparams eraHistory initialState point
    tx <- decodeTransaction transaction

    let expectedPredicateHint = expectedPredicate expected
    let utxoState = esLState (nesEs newEpochState)
    let actualPredicates =
            case manualRefScriptSizeFailure protocolParameters utxoState tx of
                Just predicateName ->
                    [predicateName]
                Nothing ->
                    case
                        applyTx
                            globals
                            (mkMempoolEnv newEpochState (pointSlotNo point))
                            (mkMempoolState newEpochState)
                            tx
                     of
                        Left err ->
                            normalizeApplyTxError expectedPredicateHint err
                        Right _ ->
                            []

    case (expected, actualPredicates) of
        (ExpectedPass, []) ->
            Right "Pass"
        (ExpectedPass, predicates) ->
            Left (ValidationMismatch "Pass" (renderActualPredicates predicates))
        (ExpectedFailure predicateName, [actualPredicate])
            | predicateName == actualPredicate ->
                Right predicateName
        (ExpectedFailure predicateName, []) ->
            Left (ValidationMismatch predicateName "Pass")
        (ExpectedFailure predicateName, predicates) ->
            Left (ValidationMismatch predicateName (renderActualPredicates predicates))

manualRefScriptSizeFailure
    :: ProtocolParametersFixture
    -> LedgerState ConwayEra
    -> Tx TopTx ConwayEra
    -> Maybe Text
manualRefScriptSizeFailure ProtocolParametersFixture{maxReferenceScriptsSize} (LedgerState utxoState _) tx
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

buildGlobals :: FixtureNetwork -> EraHistoryFixture -> PointFixture -> Either Error Globals
buildGlobals network eraHistory point = do
    let EraHistoryFixture{stabilityWindow = eraStabilityWindow} = eraHistory
    let activeSlotCoeff = defaultActiveSlotCoeff
    securityParameter <- inferSecurityParameter eraStabilityWindow activeSlotCoeff
    epochInfo <- buildEpochInfo eraHistory point

    pure
        Globals
            { epochInfo = epochInfo
            , slotsPerKESPeriod = 129600
            , stabilityWindow = eraStabilityWindow
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

buildEpochInfo :: EraHistoryFixture -> PointFixture -> Either Error (EpochInfo.EpochInfo (Either Text))
buildEpochInfo eraHistory point = do
    eraSummary <- singleEraSummary eraHistory

    startBound <- buildBound (start eraSummary)
    eraParams <- buildEraParams (params eraSummary)
    endBound <- case end eraSummary of
        Just boundedEnd ->
            EraEnd <$> buildBound boundedEnd
        Nothing -> do
            let SlotNo startSlot = boundSlot startBound
            let EpochNo startEpoch = boundEpoch startBound
            let SlotNo currentSlot = pointConsensusSlotNo point
            let EpochSize epochSize = eraEpochSize eraParams
            when (currentSlot < startSlot) $
                Left (UnsupportedFixture "the fixture point is before the era-history start bound")
            let currentEpoch = startEpoch + ((currentSlot - startSlot) `div` epochSize)
            pure (EraEnd (mkUpperBound eraParams startBound (EpochNo (currentEpoch + 1))))

    let summary :: Summary '[()]
        summary =
            Summary
                ( SOPNonEmpty.NonEmptyOne
                    EraSummary
                        { eraStart = startBound
                        , eraEnd = endBound
                        , eraParams = eraParams
                        }
                )

    pure (hoistEpochInfo (first showText . runExcept) (summaryToEpochInfo summary))

buildBound :: EraBoundFixture -> Either Error Bound
buildBound EraBoundFixture{time, slot, epoch} =
    pure
        Bound
            { boundTime = RelativeTime (fromIntegral time)
            , boundSlot = SlotNo slot
            , boundEpoch = EpochNo epoch
            , boundPerasRound = perasDisabled
            }

buildEraParams :: EraParamsFixture -> Either Error EraParams
buildEraParams EraParamsFixture{epochSizeSlots, slotLengthMs, eraName}
    | eraName /= "Conway" =
        Left (UnsupportedFixture ("unsupported era name in era history: " <> eraName))
    | otherwise =
        pure
            EraParams
                { eraEpochSize = EpochSize epochSizeSlots
                , eraSlotLength = slotLengthFromMillisec (fromIntegral slotLengthMs)
                , eraSafeZone = UnsafeIndefiniteSafeZone
                , eraGenesisWin = GenesisWindow 0
                , eraPerasRoundLength = perasDisabled
                }

inferSecurityParameter :: Word64 -> ActiveSlotCoeff -> Either Error Word64
inferSecurityParameter stabilityWindowValue activeSlotCoeff =
    maybe
        (Left (UnsupportedFixture ("could not infer the security parameter from stabilityWindow=" <> show stabilityWindowValue)))
        Right
        (find (\candidate -> computeStabilityWindow candidate activeSlotCoeff == stabilityWindowValue) [1 .. stabilityWindowValue])

buildProtocolParameters :: ProtocolParametersFixture -> Either Error (PParams ConwayEra)
buildProtocolParameters fixture = do
    let minFeeCoefficientValue = minFeeCoefficient fixture
    let minFeeConstantValue = minFeeConstant fixture
    maxBlockBodySize <- boundedIntegral @Word64 @Word32 "maxBlockBodySize" (maxBlockBodySize fixture)
    maxBlockHeaderSize <- boundedIntegral @Word64 @Word16 "maxBlockHeaderSize" (maxBlockHeaderSize fixture)
    maxTransactionSize <- boundedIntegral @Word64 @Word32 "maxTransactionSize" (maxTransactionSize fixture)
    maxValueSize <- boundedIntegral @Word64 @Word32 "maxValueSize" (maxValueSize fixture)
    let stakeCredentialDepositValue = stakeCredentialDeposit fixture
    let stakePoolDepositValue = stakePoolDeposit fixture
    stakePoolRetirementEpochBound <- boundedIntegral @Word64 @Word32 "stakePoolRetirementEpochBound" (stakePoolRetirementEpochBound fixture)
    stakePoolPledgeInfluence <- boundedRatio @NonNegativeInterval "stakePoolPledgeInfluence" (stakePoolPledgeInfluence fixture)
    let minStakePoolCostValue = minStakePoolCost fixture
    desiredNumberOfStakePools <- boundedIntegral @Word64 @Word16 "desiredNumberOfStakePools" (desiredNumberOfStakePools fixture)
    monetaryExpansion <- boundedRatio @UnitInterval "monetaryExpansion" (monetaryExpansion fixture)
    treasuryExpansion <- boundedRatio @UnitInterval "treasuryExpansion" (treasuryExpansion fixture)
    collateralPercentage <- boundedIntegral @Word64 @Word16 "collateralPercentage" (collateralPercentage fixture)
    maxCollateralInputs <- boundedIntegral @Word64 @Word16 "maxCollateralInputs" (maxCollateralInputs fixture)
    scriptExecutionPrices <- toLedgerPrices (scriptExecutionPrices fixture)
    maxExecutionUnitsPerTransaction <- toLedgerExecutionUnits "maxExecutionUnitsPerTransaction" (maxExecutionUnitsPerTransaction fixture)
    maxExecutionUnitsPerBlock <- toLedgerExecutionUnits "maxExecutionUnitsPerBlock" (maxExecutionUnitsPerBlock fixture)
    minFeeReferenceScripts <- validateReferenceScriptPricing (minFeeReferenceScripts fixture)
    poolVotingThresholds <- toPoolVotingThresholds (stakePoolVotingThresholds fixture)
    dRepVotingThresholds <- toDRepVotingThresholds (delegateRepresentativeVotingThresholds fixture)
    constitutionalCommitteeMinSize <- pure (fromIntegral (constitutionalCommitteeMinSize fixture))
    constitutionalCommitteeMaxTermLength <- boundedIntegral @Word64 @Word32 "constitutionalCommitteeMaxTermLength" (constitutionalCommitteeMaxTermLength fixture)
    governanceActionLifetime <- boundedIntegral @Word64 @Word32 "governanceActionLifetime" (governanceActionLifetime fixture)
    let governanceActionDepositValue = governanceActionDeposit fixture
    let delegateRepresentativeDepositValue = delegateRepresentativeDeposit fixture
    delegateRepresentativeMaxIdleTime <- boundedIntegral @Word64 @Word32 "delegateRepresentativeMaxIdleTime" (delegateRepresentativeMaxIdleTime fixture)
    protocolVersion <- toProtocolVersion (version fixture)
    let minUtxoDepositCoefficientValue = fromIntegral (minUtxoDepositCoefficient fixture)
    let minUtxoDepositConstantValue = minUtxoDepositConstant fixture
    case minUtxoDepositConstantValue of
        Nothing ->
            pure ()
        Just 0 ->
            pure ()
        Just constantValue ->
            Left
                ( UnsupportedFixture
                    ("minUtxoDepositConstant is not supported in Conway, but got " <> show constantValue)
                )

    pure $
        emptyPParams @ConwayEra
            & ppTxFeePerByteL .~ CoinPerByte (compactCoinOrError "minFeeCoefficient" (Coin minFeeCoefficientValue))
            & ppTxFeeFixedL .~ Coin minFeeConstantValue
            & ppMaxBBSizeL .~ maxBlockBodySize
            & ppMaxBHSizeL .~ maxBlockHeaderSize
            & ppMaxTxSizeL .~ maxTransactionSize
            & ppMaxValSizeL .~ maxValueSize
            & ppKeyDepositL .~ Coin stakeCredentialDepositValue
            & ppPoolDepositL .~ Coin stakePoolDepositValue
            & ppEMaxL .~ EpochInterval stakePoolRetirementEpochBound
            & ppA0L .~ stakePoolPledgeInfluence
            & ppMinPoolCostL .~ Coin minStakePoolCostValue
            & ppNOptL .~ desiredNumberOfStakePools
            & ppRhoL .~ monetaryExpansion
            & ppTauL .~ treasuryExpansion
            & ppCollateralPercentageL .~ collateralPercentage
            & ppMaxCollateralInputsL .~ maxCollateralInputs
            & ppPricesL .~ scriptExecutionPrices
            & ppMaxTxExUnitsL .~ maxExecutionUnitsPerTransaction
            & ppMaxBlockExUnitsL .~ maxExecutionUnitsPerBlock
            & ppPoolVotingThresholdsL .~ poolVotingThresholds
            & ppDRepVotingThresholdsL .~ dRepVotingThresholds
            & ppCommitteeMinSizeL .~ constitutionalCommitteeMinSize
            & ppCommitteeMaxTermLengthL .~ EpochInterval constitutionalCommitteeMaxTermLength
            & ppGovActionLifetimeL .~ EpochInterval governanceActionLifetime
            & ppGovActionDepositL .~ Coin governanceActionDepositValue
            & ppDRepDepositL .~ Coin delegateRepresentativeDepositValue
            & ppDRepActivityL .~ EpochInterval delegateRepresentativeMaxIdleTime
            & ppProtocolVersionL .~ protocolVersion
            & ppCoinsPerUTxOByteL .~ CoinPerByte (compactCoinOrError "minUtxoDepositCoefficient" (Coin minUtxoDepositCoefficientValue))
            & ppMinFeeRefScriptCostPerByteL .~ minFeeReferenceScripts
            & ppCostModelsL .~ plutusCostModels fixture

buildNewEpochState
    :: PParams ConwayEra
    -> EraHistoryFixture
    -> InitialStateFixture
    -> PointFixture
    -> Either Error (NewEpochState ConwayEra)
buildNewEpochState pparams eraHistory InitialStateFixture{utxo, pools, accounts, dreps, governanceActivity} point = do
    utxoEntries <- traverse toLedgerUtxoEntry utxo
    accountEntries <- traverse toLedgerAccountEntry accounts
    dRepEntries <- traverse toLedgerDRepEntry dreps
    poolIdTexts <- traverse (pure . unPoolIdFixture) pools
    currentEpoch <- pointEpochNo eraHistory point

    let accountsMap = Map.fromList accountEntries
    let dRepDelegators =
            Map.fromListWith (<>)
                [ (credential, Set.singleton stakingCredential)
                | (stakingCredential, FixtureAccountState{drep = Just delegatedDRep}) <- accountEntries
                , DRepCredential credential <- [delegatedDRep]
                ]
    let dRepStates =
            Map.fromList
                [ ( credential
                  , DRepState
                        { drepExpiry = expiry
                        , drepAnchor = SNothing
                        , drepDeposit = compactDeposit
                        , drepDelegs = Map.findWithDefault mempty credential dRepDelegators
                        }
                  )
                | (stakingCredential, FixtureDRepState{deposit = compactDeposit, expiry}) <- dRepEntries
                , let credential = coerce stakingCredential
                ]
    let poolStates =
            Map.fromList
                [ (KeyHash hashValue, defaultStakePoolState (Coin (poolDepositAmount pparams)))
                | poolIdText <- poolIdTexts
                , Just hashValue <- [hashFromTextAsHex poolIdText]
                ]
    let deposited =
            Coin
                ( sum
                    [ accountDepositValue
                    | (_, FixtureAccountState{deposit = compactDeposit}) <- accountEntries
                    , let Coin accountDepositValue = fromCompact compactDeposit
                    ]
                    + sum
                        [ dRepDeposit
                        | (_, FixtureDRepState{deposit = compactDeposit}) <- dRepEntries
                        , let Coin dRepDeposit = fromCompact compactDeposit
                        ]
                    + (fromIntegral (length poolIdTexts) * poolDepositAmount pparams)
                )
    let govState =
            emptyGovState
                & curPParamsGovStateL .~ pparams
                & prevPParamsGovStateL .~ pparams
    let chainAccountState = ChainAccountState {casTreasury = Coin 0, casReserves = Coin 0}
    let utxoState =
            UTxOState
                { utxosUtxo = UTxO (Map.fromList utxoEntries)
                , utxosDeposited = deposited
                , utxosFees = Coin 0
                , utxosGovState = govState
                , utxosInstantStake = def
                , utxosDonation = Coin 0
                }
    let certState =
            ConwayCertState
                { conwayCertVState =
                    VState
                        { vsDReps = dRepStates
                        , vsCommitteeState = def
                        , vsNumDormantEpochs = phaseOneEpochNo (consecutiveDormantEpochs governanceActivity)
                        }
                , conwayCertPState =
                    PState def poolStates def def
                , conwayCertDState =
                    DState
                        { dsAccounts =
                            ConwayAccounts
                                ( Map.map
                                    (\FixtureAccountState{balance, deposit, pool, drep} ->
                                        ConwayAccountState
                                            { casBalance = balance
                                            , casDeposit = deposit
                                            , casStakePoolDelegation =
                                                pool >>= \(PoolIdFixture poolIdText) ->
                                                    KeyHash <$> hashFromTextAsHex poolIdText
                                            , casDRepDelegation = drep
                                            }
                                    )
                                    accountsMap
                                )
                        , dsFutureGenDelegs = def
                        , dsGenDelegs = GenDelegs mempty
                        , dsIRewards = def
                        }
                }
    let ledgerState = LedgerState utxoState certState
    let epochState =
            EpochState
                { esChainAccountState = chainAccountState
                , esLState = ledgerState
                , esSnapshots = def
                , esNonMyopic = def
                }

    pure
        NewEpochState
            { nesEL = currentEpoch
            , nesBprev = def
            , nesBcur = def
            , nesEs = epochState
            , nesRu = SNothing
            , nesPd = def
            , stashedAVVMAddresses = ()
            }

poolDepositAmount :: PParams ConwayEra -> Integer
poolDepositAmount pparams =
    case pparams ^. ppPoolDepositL of
        Coin amount ->
            amount

defaultStakePoolState :: Coin -> StakePoolState
defaultStakePoolState depositCoin =
    def
        & spsDepositL .~ compactCoinOrError "stakePoolDeposit" depositCoin

pointEpochNo :: EraHistoryFixture -> PointFixture -> Either Error LedgerSlot.EpochNo
pointEpochNo eraHistory point = do
    EraSummaryFixture
        { start = EraBoundFixture{slot = eraStartSlot, epoch = eraStartEpoch}
        , params = EraParamsFixture{epochSizeSlots}
        } <- singleEraSummary eraHistory
    let PointFixture{slot = currentSlot} = point
    when (currentSlot < eraStartSlot) $
        Left (UnsupportedFixture "the fixture point is before the era-history start bound")
    pure
        (LedgerSlot.EpochNo (eraStartEpoch + ((currentSlot - eraStartSlot) `div` epochSizeSlots)))

toLedgerUtxoEntry :: UtxoEntryFixture -> Either Error (TxIn, TxOut ConwayEra)
toLedgerUtxoEntry UtxoEntryFixture{input, output} =
    Right (input, output)

toLedgerAccountEntry
    :: FixtureAccount
    -> Either Error (Credential Staking, FixtureAccountState)
toLedgerAccountEntry FixtureAccount{credential, deposit, rewards, pool, drep} = do
    compactDeposit <- compactCoin "account deposit" (Coin deposit)
    compactBalance <- compactCoin "account rewards" (Coin rewards)
    pure
        ( unStakingCredentialFixture credential
        , FixtureAccountState
            { balance = compactBalance
            , deposit = compactDeposit
            , pool = poolId <$> pool
            , drep = voteDelegationId <$> drep
            }
        )

toLedgerDRepEntry
    :: FixtureDRepEntry
    -> Either Error (Credential Staking, FixtureDRepState)
toLedgerDRepEntry FixtureDRepEntry{credential, deposit, validUntil} = do
    compactDeposit <- compactCoin "delegate representative deposit" (Coin deposit)
    pure
        ( unStakingCredentialFixture credential
        , FixtureDRepState
            { deposit = compactDeposit
            , expiry = phaseOneEpochNo validUntil
            }
        )

phaseOneEpochNo :: Word64 -> LedgerSlot.EpochNo
phaseOneEpochNo =
    LedgerSlot.EpochNo

perasDisabled :: forall a. PerasEnabled a
perasDisabled =
    NoPerasEnabled

compactCoin :: Text -> Coin -> Either Error (CompactForm Coin)
compactCoin contextLabel coinValue =
    maybe
        (Left (UnsupportedFixture ("cannot compact " <> contextLabel <> ": " <> show coinValue)))
        Right
        (toCompact coinValue)

compactCoinOrError :: Text -> Coin -> CompactForm Coin
compactCoinOrError contextLabel coinValue =
    case compactCoin contextLabel coinValue of
        Right compactValue ->
            compactValue
        Left _ ->
            error ("Invalid compact coin for " <> contextLabel)

toLedgerPrices :: ScriptExecutionPricesFixture -> Either Error Prices
toLedgerPrices ScriptExecutionPricesFixture{memory, cpu} =
    Prices
        <$> boundedRatio @NonNegativeInterval "scriptExecutionPrices.memory" memory
        <*> boundedRatio @NonNegativeInterval "scriptExecutionPrices.cpu" cpu

toLedgerExecutionUnits :: Text -> ExecutionUnitsFixture -> Either Error ExUnits
toLedgerExecutionUnits _ ExecutionUnitsFixture{memory, cpu} =
    pure (ExUnits (fromIntegral memory) (fromIntegral cpu))

validateReferenceScriptPricing :: MinFeeReferenceScriptsFixture -> Either Error NonNegativeInterval
validateReferenceScriptPricing MinFeeReferenceScriptsFixture{range, base, multiplier} = do
    when (range /= 25600) $
        Left
            ( UnsupportedFixture
                ("minFeeReferenceScripts.range must be 25600 for the current Haskell ledger, but got " <> show range)
            )
    when (ratioToRational multiplier /= (12 % 10)) $
        Left
            ( UnsupportedFixture
                ( "minFeeReferenceScripts.multiplier must be 12/10 for the current Haskell ledger, but got "
                    <> renderRatio multiplier
                )
            )
    boundedRatio @NonNegativeInterval "minFeeReferenceScripts.base" base

toPoolVotingThresholds :: PoolVotingThresholdsFixture -> Either Error PoolVotingThresholds
toPoolVotingThresholds PoolVotingThresholdsFixture{noConfidence, constitutionalCommittee, hardForkInitiation, protocolParametersUpdate} =
    PoolVotingThresholds
        <$> boundedRatio @UnitInterval "stakePoolVotingThresholds.noConfidence" noConfidence
        <*> boundedRatio @UnitInterval
            "stakePoolVotingThresholds.constitutionalCommittee.default"
            (defaultThreshold constitutionalCommittee)
        <*> boundedRatio @UnitInterval
            "stakePoolVotingThresholds.constitutionalCommittee.stateOfNoConfidence"
            (stateOfNoConfidence constitutionalCommittee)
        <*> boundedRatio @UnitInterval "stakePoolVotingThresholds.hardForkInitiation" hardForkInitiation
        <*> boundedRatio @UnitInterval
            "stakePoolVotingThresholds.protocolParametersUpdate.security"
            (security protocolParametersUpdate)

toDRepVotingThresholds :: DRepVotingThresholdsFixture -> Either Error DRepVotingThresholds
toDRepVotingThresholds
    DRepVotingThresholdsFixture
        { noConfidence
        , constitution
        , constitutionalCommittee
        , hardForkInitiation
        , protocolParametersUpdate =
            DRepProtocolParametersUpdateThresholdsFixture
                { network = protocolParametersUpdateNetwork
                , economic = protocolParametersUpdateEconomic
                , technical = protocolParametersUpdateTechnical
                , governance = protocolParametersUpdateGovernance
                }
        , treasuryWithdrawals
        } =
        DRepVotingThresholds
            <$> boundedRatio @UnitInterval "delegateRepresentativeVotingThresholds.noConfidence" noConfidence
            <*> boundedRatio @UnitInterval
                "delegateRepresentativeVotingThresholds.constitutionalCommittee.default"
                (defaultThreshold constitutionalCommittee)
            <*> boundedRatio @UnitInterval
                "delegateRepresentativeVotingThresholds.constitutionalCommittee.stateOfNoConfidence"
                (stateOfNoConfidence constitutionalCommittee)
            <*> boundedRatio @UnitInterval "delegateRepresentativeVotingThresholds.constitution" constitution
            <*> boundedRatio @UnitInterval "delegateRepresentativeVotingThresholds.hardForkInitiation" hardForkInitiation
            <*> boundedRatio @UnitInterval
                "delegateRepresentativeVotingThresholds.protocolParametersUpdate.network"
                protocolParametersUpdateNetwork
            <*> boundedRatio @UnitInterval
                "delegateRepresentativeVotingThresholds.protocolParametersUpdate.economic"
                protocolParametersUpdateEconomic
            <*> boundedRatio @UnitInterval
                "delegateRepresentativeVotingThresholds.protocolParametersUpdate.technical"
                protocolParametersUpdateTechnical
            <*> boundedRatio @UnitInterval
                "delegateRepresentativeVotingThresholds.protocolParametersUpdate.governance"
                protocolParametersUpdateGovernance
            <*> boundedRatio @UnitInterval
                "delegateRepresentativeVotingThresholds.treasuryWithdrawals"
                treasuryWithdrawals

toProtocolVersion :: ProtocolVersionFixture -> Either Error ProtVer
toProtocolVersion ProtocolVersionFixture{major, minor} = do
    majorVersion <-
        maybe
            (Left (UnsupportedFixture ("protocol version major is out of bounds: " <> showText major)))
            Right
            (mkVersion major :: Maybe Version)
    pure (ProtVer majorVersion (fromIntegral minor))

boundedIntegral :: forall a b. (Integral a, Integral b, Bounded b, Show a) => Text -> a -> Either Error b
boundedIntegral contextLabel value =
    maybe
        (Left (UnsupportedFixture (contextLabel <> " is out of bounds: " <> showText value)))
        Right
        (integralToBounded @a @b @Maybe value)

showText :: Show a => a -> Text
showText value =
    toText (show value :: String)

boundedRatio :: forall r. BoundedRational r => Text -> RatioFixture -> Either Error r
boundedRatio contextLabel ratioValue =
    maybe
        (Left (UnsupportedFixture (contextLabel <> " is outside the supported bounds: " <> renderRatio ratioValue)))
        Right
        (boundRational (ratioToRational ratioValue))

renderRatio :: RatioFixture -> Text
renderRatio RatioFixture{numerator = numer, denominator = denom} =
    show numer <> "/" <> show denom

ratioToRational :: RatioFixture -> Rational
ratioToRational RatioFixture{numerator = numer, denominator = denom} =
    numer % denom

decodeTransaction :: Text -> Either Error (Tx TopTx ConwayEra)
decodeTransaction hexText =
    first (UnsupportedFixture . ("cannot decode transaction CBOR: " <>) . showText) $
        withHexText
            (\bytes -> decodeFullAnnotator (eraProtVerLow @ConwayEra) "Tx" decCBOR (LazyByteString.fromStrict bytes))
            hexText

expectedPredicate :: ExpectedFixture -> Maybe Text
expectedPredicate = \case
    ExpectedPass ->
        Nothing
    ExpectedFailure predicateName ->
        Just predicateName

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
        "unsupported:" <> show otherFailure

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
        "unsupported:" <> show expectedHint <> ":" <> show otherFailure

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
        "unsupported:" <> show otherFailure

normalizeCertsFailure :: Maybe Text -> ConwayCertsPredFailure ConwayEra -> Text
normalizeCertsFailure expectedHint = \case
    CertFailure failure ->
        normalizeCertFailure expectedHint failure
    otherFailure ->
        "unsupported:" <> show otherFailure

normalizeCertFailure :: Maybe Text -> ConwayCertPredFailure ConwayEra -> Text
normalizeCertFailure expectedHint = \case
    DelegFailure failure ->
        normalizeDelegFailure expectedHint failure
    GovCertFailure failure ->
        normalizeGovCertFailure failure
    otherFailure ->
        "unsupported:" <> show otherFailure

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
        "unsupported:" <> show otherFailure

normalizeGovCertFailure :: ConwayGovCertPredFailure ConwayEra -> Text
normalizeGovCertFailure = \case
    ConwayDRepAlreadyRegistered{} ->
        "DRepAlreadyRegistered"
    otherFailure ->
        "unsupported:" <> show otherFailure

fixtureNetworkId :: FixtureNetwork -> Network
fixtureNetworkId (FixtureNetwork networkName)
    | networkName == "mainnet" =
        Mainnet
    | otherwise =
        Testnet

defaultActiveSlotCoeff :: ActiveSlotCoeff
defaultActiveSlotCoeff =
    case boundRational (1 % 20) of
        Just value ->
            mkActiveSlotCoeff value
        Nothing ->
            error "Invalid ActiveSlotCoeff placeholder in Command.ValidatePhaseOne.Run"

pointSlotNo :: PointFixture -> LedgerSlot.SlotNo
pointSlotNo PointFixture{slot} =
    LedgerSlot.SlotNo slot

pointConsensusSlotNo :: PointFixture -> SlotNo
pointConsensusSlotNo PointFixture{slot} =
    SlotNo slot

data ResolvedFixture = ResolvedFixture
    { network :: !FixtureNetwork
    , eraHistory :: !EraHistoryFixture
    , protocolParameters :: !ProtocolParametersFixture
    , initialState :: !InitialStateFixture
    , point :: !PointFixture
    , transaction :: !Text
    , expected :: !ExpectedFixture
    }
    deriving stock (Generic)

instance FromJSON ResolvedFixture

newtype FixtureNetwork = FixtureNetwork
    Text

instance FromJSON FixtureNetwork where
    parseJSON =
        withText "FixtureNetwork" $ \networkName ->
            if networkName == "mainnet"
                || networkName == "preprod"
                || networkName == "preview"
                || "testnet_" `Text.isPrefixOf` networkName
                then pure (FixtureNetwork networkName)
                else fail ("Unsupported network: " <> toString networkName)

data EraHistoryFixture = EraHistoryFixture
    { stabilityWindow :: !Word64
    , eras :: ![EraSummaryFixture]
    }
    deriving stock (Generic)

instance FromJSON EraHistoryFixture

data EraSummaryFixture = EraSummaryFixture
    { start :: !EraBoundFixture
    , end :: !(Maybe EraBoundFixture)
    , params :: !EraParamsFixture
    }
    deriving stock (Generic)

instance FromJSON EraSummaryFixture

data EraBoundFixture = EraBoundFixture
    { time :: !Word64
    , slot :: !Word64
    , epoch :: !Word64
    }
    deriving stock (Generic)

instance FromJSON EraBoundFixture

data EraParamsFixture = EraParamsFixture
    { epochSizeSlots :: !Word64
    , slotLengthMs :: !Word64
    , eraName :: !Text
    }
    deriving stock (Generic)

instance FromJSON EraParamsFixture

data InitialStateFixture = InitialStateFixture
    { utxo :: ![UtxoEntryFixture]
    , pools :: ![PoolIdFixture]
    , accounts :: ![FixtureAccount]
    , dreps :: ![FixtureDRepEntry]
    , governanceActivity :: !GovernanceActivityFixture
    }
    deriving stock (Generic)

instance FromJSON InitialStateFixture where
    parseJSON =
        withObject "InitialStateFixture" $ \objectValue ->
            InitialStateFixture
                <$> objectValue .:? "utxo" .!= []
                <*> objectValue .:? "pools" .!= []
                <*> objectValue .:? "accounts" .!= []
                <*> objectValue .:? "dreps" .!= []
                <*> objectValue .: "governanceActivity"

data UtxoEntryFixture = UtxoEntryFixture
    { input :: !TxIn
    , output :: !(TxOut ConwayEra)
    }

instance FromJSON UtxoEntryFixture where
    parseJSON =
        withObject "UtxoEntryFixture" $ \objectValue ->
            UtxoEntryFixture
                <$> (objectValue .: "input" >>= parseCborHex "TxIn")
                <*> (objectValue .: "output" >>= parseCborHex "TxOut")

newtype StakingCredentialFixture = StakingCredentialFixture
    { unStakingCredentialFixture :: Credential Staking
    }

instance FromJSON StakingCredentialFixture where
    parseJSON value =
        StakingCredentialFixture <$> (parseJSON value >>= parseCborHex "StakeCredential")

newtype PoolIdFixture = PoolIdFixture
    { unPoolIdFixture :: Text
    }

instance FromJSON PoolIdFixture where
    parseJSON =
        withText "PoolIdFixture" $ \hexText ->
            if Text.length hexText == 56 && Text.all isHexDigit hexText
                then pure (PoolIdFixture hexText)
                else fail ("Invalid pool id hex: " <> toString hexText)

data FixtureAccount = FixtureAccount
    { credential :: !StakingCredentialFixture
    , deposit :: !Integer
    , rewards :: !Integer
    , pool :: !(Maybe PoolDelegationFixture)
    , drep :: !(Maybe VoteDelegationFixture)
    }
    deriving stock (Generic)

instance FromJSON FixtureAccount where
    parseJSON =
        withObject "FixtureAccount" $ \objectValue ->
            FixtureAccount
                <$> objectValue .: "credential"
                <*> objectValue .: "deposit"
                <*> objectValue .:? "rewards" .!= 0
                <*> objectValue .:? "pool"
                <*> objectValue .:? "drep"

data PoolDelegationFixture = PoolDelegationFixture
    { poolId :: !PoolIdFixture
    }

instance FromJSON PoolDelegationFixture where
    parseJSON =
        withObject "PoolDelegationFixture" $ \objectValue ->
            PoolDelegationFixture <$> objectValue .: "id"

data VoteDelegationFixture = VoteDelegationFixture
    { voteDelegationId :: !DRep
    }

instance FromJSON VoteDelegationFixture where
    parseJSON =
        withObject "VoteDelegationFixture" $ \objectValue ->
            VoteDelegationFixture
                <$> (objectValue .: "id" >>= parseCborHex "DRep")

data FixtureDRepEntry = FixtureDRepEntry
    { credential :: !StakingCredentialFixture
    , deposit :: !Integer
    , validUntil :: !Word64
    }
    deriving stock (Generic)

instance FromJSON FixtureDRepEntry

data GovernanceActivityFixture = GovernanceActivityFixture
    { consecutiveDormantEpochs :: !Word64
    }
    deriving stock (Generic)

instance FromJSON GovernanceActivityFixture

data PointFixture = PointFixture
    { slot :: !Word64
    }
    deriving stock (Generic)

instance FromJSON PointFixture

data ExpectedFixture
    = ExpectedPass
    | ExpectedFailure !Text

instance FromJSON ExpectedFixture where
    parseJSON = \case
        String "Pass" ->
            pure ExpectedPass
        Object objectValue ->
            ExpectedFailure <$> objectValue .: "predicate"
        otherValue ->
            fail ("Expected \"Pass\" or { predicate: ... }, but got " <> show otherValue)

data ProtocolParametersFixture = ProtocolParametersFixture
    { minFeeCoefficient :: !Integer
    , minFeeConstant :: !Integer
    , minFeeReferenceScripts :: !MinFeeReferenceScriptsFixture
    , minUtxoDepositConstant :: !(Maybe Integer)
    , minUtxoDepositCoefficient :: !Word64
    , maxBlockBodySize :: !Word64
    , maxBlockHeaderSize :: !Word64
    , maxTransactionSize :: !Word64
    , maxValueSize :: !Word64
    , maxReferenceScriptsSize :: !Word64
    , stakeCredentialDeposit :: !Integer
    , stakePoolDeposit :: !Integer
    , stakePoolRetirementEpochBound :: !Word64
    , stakePoolPledgeInfluence :: !RatioFixture
    , minStakePoolCost :: !Integer
    , desiredNumberOfStakePools :: !Word64
    , monetaryExpansion :: !RatioFixture
    , treasuryExpansion :: !RatioFixture
    , collateralPercentage :: !Word64
    , maxCollateralInputs :: !Word64
    , scriptExecutionPrices :: !ScriptExecutionPricesFixture
    , maxExecutionUnitsPerTransaction :: !ExecutionUnitsFixture
    , maxExecutionUnitsPerBlock :: !ExecutionUnitsFixture
    , stakePoolVotingThresholds :: !PoolVotingThresholdsFixture
    , delegateRepresentativeVotingThresholds :: !DRepVotingThresholdsFixture
    , constitutionalCommitteeMinSize :: !Word64
    , constitutionalCommitteeMaxTermLength :: !Word64
    , governanceActionLifetime :: !Word64
    , governanceActionDeposit :: !Integer
    , delegateRepresentativeDeposit :: !Integer
    , delegateRepresentativeMaxIdleTime :: !Word64
    , version :: !ProtocolVersionFixture
    , plutusCostModels :: !CostModels
    }
    deriving stock (Generic)

instance FromJSON ProtocolParametersFixture

data MinFeeReferenceScriptsFixture = MinFeeReferenceScriptsFixture
    { range :: !Word64
    , base :: !RatioFixture
    , multiplier :: !RatioFixture
    }
    deriving stock (Generic)

instance FromJSON MinFeeReferenceScriptsFixture

data ScriptExecutionPricesFixture = ScriptExecutionPricesFixture
    { memory :: !RatioFixture
    , cpu :: !RatioFixture
    }
    deriving stock (Generic)

instance FromJSON ScriptExecutionPricesFixture

data ExecutionUnitsFixture = ExecutionUnitsFixture
    { memory :: !Word64
    , cpu :: !Word64
    }
    deriving stock (Generic)

instance FromJSON ExecutionUnitsFixture

data PoolVotingThresholdsFixture = PoolVotingThresholdsFixture
    { noConfidence :: !RatioFixture
    , constitutionalCommittee :: !ConstitutionalCommitteeThresholdsFixture
    , hardForkInitiation :: !RatioFixture
    , protocolParametersUpdate :: !PoolProtocolParametersUpdateThresholdsFixture
    }
    deriving stock (Generic)

instance FromJSON PoolVotingThresholdsFixture

data PoolProtocolParametersUpdateThresholdsFixture = PoolProtocolParametersUpdateThresholdsFixture
    { security :: !RatioFixture
    }
    deriving stock (Generic)

instance FromJSON PoolProtocolParametersUpdateThresholdsFixture

data DRepVotingThresholdsFixture = DRepVotingThresholdsFixture
    { noConfidence :: !RatioFixture
    , constitution :: !RatioFixture
    , constitutionalCommittee :: !ConstitutionalCommitteeThresholdsFixture
    , hardForkInitiation :: !RatioFixture
    , protocolParametersUpdate :: !DRepProtocolParametersUpdateThresholdsFixture
    , treasuryWithdrawals :: !RatioFixture
    }
    deriving stock (Generic)

instance FromJSON DRepVotingThresholdsFixture

data DRepProtocolParametersUpdateThresholdsFixture = DRepProtocolParametersUpdateThresholdsFixture
    { network :: !RatioFixture
    , economic :: !RatioFixture
    , technical :: !RatioFixture
    , governance :: !RatioFixture
    }
    deriving stock (Generic)

instance FromJSON DRepProtocolParametersUpdateThresholdsFixture

data ConstitutionalCommitteeThresholdsFixture = ConstitutionalCommitteeThresholdsFixture
    { defaultThreshold :: !RatioFixture
    , stateOfNoConfidence :: !RatioFixture
    }

instance FromJSON ConstitutionalCommitteeThresholdsFixture where
    parseJSON =
        withObject "ConstitutionalCommitteeThresholdsFixture" $ \objectValue ->
            ConstitutionalCommitteeThresholdsFixture
                <$> objectValue .: "default"
                <*> objectValue .: "stateOfNoConfidence"

data ProtocolVersionFixture = ProtocolVersionFixture
    { major :: !Word64
    , minor :: !Word64
    }
    deriving stock (Generic)

instance FromJSON ProtocolVersionFixture

data RatioFixture = RatioFixture
    { numerator :: !Integer
    , denominator :: !Integer
    }
    deriving stock (Generic)

instance FromJSON RatioFixture

data FixtureAccountState = FixtureAccountState
    { balance :: !(CompactForm Coin)
    , deposit :: !(CompactForm Coin)
    , pool :: !(Maybe PoolIdFixture)
    , drep :: !(Maybe DRep)
    }

data FixtureDRepState = FixtureDRepState
    { deposit :: !(CompactForm Coin)
    , expiry :: !LedgerSlot.EpochNo
    }

singleEraSummary :: EraHistoryFixture -> Either Error EraSummaryFixture
singleEraSummary EraHistoryFixture{eras = [singleEra]} =
    Right singleEra
singleEraSummary EraHistoryFixture{} =
    Left (UnsupportedFixture "only single-era Conway phase-one fixtures are supported")

parseCborHex :: forall a. DecCBOR a => Text -> Text -> Parser a
parseCborHex contextLabel hexText =
    case withHexText (decodeFull' (eraProtVerLow @ConwayEra)) hexText of
        Right value ->
            pure value
        Left err ->
            fail (toString (contextLabel <> " failed to decode from CBOR hex: " <> showText err))
