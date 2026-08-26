{-# LANGUAGE DataKinds #-}
{-# LANGUAGE ScopedTypeVariables #-}

module Data.Fixture.ProtocolParameters
    ( ProtocolParameters (..)
    , protocolParametersFromJson
    ) where

import Relude

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
    ( BoundedRational
    , EpochInterval (..)
    , NonNegativeInterval
    , ProtVer (..)
    )
import Cardano.Ledger.Binary.Version
    ( Version
    , mkVersion
    )
import Cardano.Ledger.Coin
    ( Coin (..)
    )
import Cardano.Ledger.Conway
    ( ConwayEra
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
import Cardano.Ledger.Plutus.CostModels
    ( CostModels
    , mkCostModel
    , mkCostModels
    )
import Cardano.Ledger.Plutus.ExUnits
    ( ExUnits (..)
    , Prices (..)
    )
import Cardano.Ledger.Plutus.Language
    ( Language (..)
    )
import Data.Aeson
    ( Object
    , Value
    , withObject
    , (.:)
    , (.:?)
    )
import Data.Aeson.Key
    ( Key
    )
import Data.Aeson.Types
    ( Parser
    )
import Data.Fixture.Common
    ( boundedIntegral
    , boundedRatio
    , compactCoinOrError
    , ratioFromJson
    , renderRatio
    )
import Data.Ratio
    ( (%)
    )
import Lens.Micro
    ( (.~)
    )

import qualified Data.Aeson.Key as Key
import qualified Data.Map.Strict as Map

data ProtocolParameters = ProtocolParameters
    { maxReferenceScriptsSize :: !Word64
    , pparams :: !(PParams ConwayEra)
    }

protocolParametersFromJson :: Value -> Parser ProtocolParameters
protocolParametersFromJson =
    withObject "ProtocolParameters" $ \objectValue -> do
        minFeeCoefficientValue <- objectValue .: "min_fee_coefficient"
        minFeeConstantValue <- objectValue .: "min_fee_constant"
        maxBlockBodySize <- parseBounded objectValue "max_block_body_size"
        maxBlockHeaderSize <- parseBounded objectValue "max_block_header_size"
        maxTransactionSize <- parseBounded objectValue "max_transaction_size"
        maxValueSize <- parseBounded objectValue "max_value_size"
        maxReferenceScriptsSize <- objectValue .: "max_reference_scripts_size"
        stakeCredentialDepositValue <- objectValue .: "stake_credential_deposit"
        stakePoolDepositValue <- objectValue .: "stake_pool_deposit"
        stakePoolRetirementEpochBound <- parseBounded objectValue "stake_pool_retirement_epoch_bound"
        stakePoolPledgeInfluence <- objectValue .: "stake_pool_pledge_influence" >>= parseBoundedRatio "stake_pool_pledge_influence"
        minStakePoolCostValue <- objectValue .: "min_stake_pool_cost"
        desiredNumberOfStakePools <- parseBounded objectValue "desired_number_of_stake_pools"
        monetaryExpansion <- objectValue .: "monetary_expansion" >>= parseBoundedRatio "monetary_expansion"
        treasuryExpansion <- objectValue .: "treasury_expansion" >>= parseBoundedRatio "treasury_expansion"
        collateralPercentage <- parseBounded objectValue "collateral_percentage"
        maxCollateralInputs <- parseBounded objectValue "max_collateral_inputs"
        scriptExecutionPrices <- objectValue .: "script_execution_prices" >>= pricesFromJson
        maxExecutionUnitsPerTransaction <- objectValue .: "max_execution_units_per_transaction" >>= executionUnitsFromJson
        maxExecutionUnitsPerBlock <- objectValue .: "max_execution_units_per_block" >>= executionUnitsFromJson
        minFeeReferenceScripts <- objectValue .: "min_fee_reference_scripts" >>= minFeeReferenceScriptsFromJson
        poolVotingThresholds <- objectValue .: "stake_pool_voting_thresholds" >>= poolVotingThresholdsFromJson
        dRepVotingThresholds <- objectValue .: "delegate_representative_voting_thresholds" >>= drepVotingThresholdsFromJson
        constitutionalCommitteeMinSize <- fromIntegral <$> (objectValue .: "constitutional_committee_min_size" :: Parser Word64)
        constitutionalCommitteeMaxTermLength <- parseBounded objectValue "constitutional_committee_max_term_length"
        governanceActionLifetime <- parseBounded objectValue "governance_action_lifetime"
        governanceActionDepositValue <- objectValue .: "governance_action_deposit"
        delegateRepresentativeDepositValue <- objectValue .: "delegate_representative_deposit"
        delegateRepresentativeMaxIdleTime <- parseBounded objectValue "delegate_representative_max_idle_time"
        protocolVersion <- objectValue .: "version" >>= protocolVersionFromJson
        minUtxoDepositCoefficientValue <- fromIntegral <$> (objectValue .: "min_utxo_deposit_coefficient" :: Parser Word64)
        minUtxoDepositConstantValue <- (objectValue .:? "min_utxo_deposit_constant" :: Parser (Maybe Integer))
        plutusCostModels <- objectValue .: "plutus_cost_models" >>= plutusCostModelsFromJson

        case minUtxoDepositConstantValue of
            Nothing ->
                pure ()
            Just 0 ->
                pure ()
            Just constantValue ->
                fail ("minUtxoDepositConstant is not supported in Conway, but got " <> show constantValue)

        pure
            ProtocolParameters
                { maxReferenceScriptsSize
                , pparams =
                    emptyPParams @ConwayEra
                        & ppTxFeePerByteL .~ CoinPerByte (compactCoinOrError "min_fee_coefficient" (Coin minFeeCoefficientValue))
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
                        & ppCoinsPerUTxOByteL .~ CoinPerByte (compactCoinOrError "min_utxo_deposit_coefficient" (Coin minUtxoDepositCoefficientValue))
                        & ppMinFeeRefScriptCostPerByteL .~ minFeeReferenceScripts
                        & ppCostModelsL .~ plutusCostModels
                }

-- | Parse the Plutus cost models from JSON. The JSON object is expected to have keys "plutus_v1", "plutus_v2", and "plutus_v3",
--   each containing the cost model parameters for the respective Plutus version.
--   If any of the cost models are invalid, a parsing error will be raised.
plutusCostModelsFromJson :: Value -> Parser CostModels
plutusCostModelsFromJson =
    withObject "plutus_cost_models" $ \objectValue -> do
        plutusV1 <- objectValue .:? "plutus_v1"
        plutusV2 <- objectValue .:? "plutus_v2"
        plutusV3 <- objectValue .:? "plutus_v3"
        costModels <-
            sequence
                [ buildCostModel language parameters
                | (language, Just parameters) <-
                    [(PlutusV1, plutusV1), (PlutusV2, plutusV2), (PlutusV3, plutusV3)]
                ]
        pure (mkCostModels (Map.fromList costModels))
  where
    buildCostModel language parameters =
        case mkCostModel language parameters of
            Left err ->
                fail ("invalid " <> show language <> " cost model: " <> show err)
            Right costModel ->
                pure (language, costModel)

parseBounded :: forall a. (Bounded a, Integral a) => Object -> Key -> Parser a
parseBounded objectValue fieldName = do
    rawValue <- (objectValue .: fieldName :: Parser Integer)
    either fail pure (first toString (boundedIntegral (Key.toText fieldName) rawValue))

parseBoundedRatio :: forall r. BoundedRational r => Text -> Value -> Parser r
parseBoundedRatio contextLabel value = do
    rationalValue <- ratioFromJson value
    either fail pure (first toString (boundedRatio contextLabel rationalValue))

pricesFromJson :: Value -> Parser Prices
pricesFromJson =
    withObject "ScriptExecutionPrices" $ \objectValue ->
        Prices
            <$> (objectValue .: "mem_price" >>= parseBoundedRatio "script_execution_prices.mem_price")
            <*> (objectValue .: "step_price" >>= parseBoundedRatio "script_execution_prices.step_price")

executionUnitsFromJson :: Value -> Parser ExUnits
executionUnitsFromJson =
    withObject "ExecutionUnits" $ \objectValue ->
        ExUnits
            <$> (fromIntegral <$> (objectValue .: "mem" :: Parser Word64))
            <*> (fromIntegral <$> (objectValue .: "steps" :: Parser Word64))

minFeeReferenceScriptsFromJson :: Value -> Parser NonNegativeInterval
minFeeReferenceScriptsFromJson =
    withObject "MinFeeReferenceScripts" $ \objectValue -> do
        range <- objectValue .: "range"
        base <- objectValue .: "base" >>= parseBoundedRatio "min_fee_reference_scripts.base"
        multiplier <- objectValue .: "multiplier" >>= ratioFromJson
        when (range /= (25600 :: Word64)) $
            fail ("min_fee_reference_scripts.range must be 25600 for the current Haskell ledger, but got " <> show range)
        when (multiplier /= (12 % 10)) $
            fail
                ( "min_fee_reference_scripts.multiplier must be 12/10 for the current Haskell ledger, but got "
                    <> toString (renderRatio multiplier)
                )
        pure base

poolVotingThresholdsFromJson :: Value -> Parser PoolVotingThresholds
poolVotingThresholdsFromJson =
    withObject "PoolVotingThresholds" $ \objectValue ->
        PoolVotingThresholds
            <$> (objectValue .: "motion_no_confidence" >>= parseBoundedRatio "stake_pool_voting_thresholds.motion_no_confidence")
            <*> (objectValue .: "committee_normal" >>= parseBoundedRatio "stake_pool_voting_thresholds.committee_normal")
            <*> (objectValue .: "committee_no_confidence" >>= parseBoundedRatio "stake_pool_voting_thresholds.committee_no_confidence")
            <*> (objectValue .: "hard_fork_initiation" >>= parseBoundedRatio "stake_pool_voting_thresholds.hard_fork_initiation")
            <*> (objectValue .: "security_voting_threshold" >>= parseBoundedRatio "stake_pool_voting_thresholds.security_voting_threshold")

drepVotingThresholdsFromJson :: Value -> Parser DRepVotingThresholds
drepVotingThresholdsFromJson =
    withObject "DRepVotingThresholds" $ \objectValue ->
        DRepVotingThresholds
            <$> (objectValue .: "motion_no_confidence" >>= parseBoundedRatio "delegate_representative_voting_thresholds.motion_no_confidence")
            <*> (objectValue .: "committee_normal" >>= parseBoundedRatio "delegate_representative_voting_thresholds.committee_normal")
            <*> (objectValue .: "committee_no_confidence" >>= parseBoundedRatio "delegate_representative_voting_thresholds.committee_no_confidence")
            <*> (objectValue .: "update_constitution" >>= parseBoundedRatio "delegate_representative_voting_thresholds.update_constitution")
            <*> (objectValue .: "hard_fork_initiation" >>= parseBoundedRatio "delegate_representative_voting_thresholds.hard_fork_initiation")
            <*> (objectValue .: "pp_network_group" >>= parseBoundedRatio "delegate_representative_voting_thresholds.pp_network_group")
            <*> (objectValue .: "pp_economic_group" >>= parseBoundedRatio "delegate_representative_voting_thresholds.pp_economic_group")
            <*> (objectValue .: "pp_technical_group" >>= parseBoundedRatio "delegate_representative_voting_thresholds.pp_technical_group")
            <*> (objectValue .: "pp_governance_group" >>= parseBoundedRatio "delegate_representative_voting_thresholds.pp_governance_group")
            <*> (objectValue .: "treasury_withdrawal" >>= parseBoundedRatio "delegate_representative_voting_thresholds.treasury_withdrawal")

protocolVersionFromJson :: Value -> Parser ProtVer
protocolVersionFromJson =
    withObject "ProtocolVersion" $ \objectValue -> do
        major <- (objectValue .: "major" :: Parser Word64)
        minor <- (objectValue .: "minor" :: Parser Word64)
        majorVersion <-
            maybe
                (fail ("protocol version major is out of bounds: " <> show major))
                pure
                (mkVersion major :: Maybe Version)
        pure (ProtVer majorVersion (fromIntegral minor))
