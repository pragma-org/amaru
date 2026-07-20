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
import Cardano.Ledger.Plutus.ExUnits
    ( ExUnits (..)
    , Prices (..)
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

data ProtocolParameters = ProtocolParameters
    { maxReferenceScriptsSize :: !Word64
    , pparams :: !(PParams ConwayEra)
    }

protocolParametersFromJson :: Value -> Parser ProtocolParameters
protocolParametersFromJson =
    withObject "ProtocolParameters" $ \objectValue -> do
        minFeeCoefficientValue <- objectValue .: "minFeeCoefficient"
        minFeeConstantValue <- objectValue .: "minFeeConstant"
        maxBlockBodySize <- parseBounded objectValue "maxBlockBodySize"
        maxBlockHeaderSize <- parseBounded objectValue "maxBlockHeaderSize"
        maxTransactionSize <- parseBounded objectValue "maxTransactionSize"
        maxValueSize <- parseBounded objectValue "maxValueSize"
        maxReferenceScriptsSize <- objectValue .: "maxReferenceScriptsSize"
        stakeCredentialDepositValue <- objectValue .: "stakeCredentialDeposit"
        stakePoolDepositValue <- objectValue .: "stakePoolDeposit"
        stakePoolRetirementEpochBound <- parseBounded objectValue "stakePoolRetirementEpochBound"
        stakePoolPledgeInfluence <- objectValue .: "stakePoolPledgeInfluence" >>= parseBoundedRatio "stakePoolPledgeInfluence"
        minStakePoolCostValue <- objectValue .: "minStakePoolCost"
        desiredNumberOfStakePools <- parseBounded objectValue "desiredNumberOfStakePools"
        monetaryExpansion <- objectValue .: "monetaryExpansion" >>= parseBoundedRatio "monetaryExpansion"
        treasuryExpansion <- objectValue .: "treasuryExpansion" >>= parseBoundedRatio "treasuryExpansion"
        collateralPercentage <- parseBounded objectValue "collateralPercentage"
        maxCollateralInputs <- parseBounded objectValue "maxCollateralInputs"
        scriptExecutionPrices <- objectValue .: "scriptExecutionPrices" >>= pricesFromJson
        maxExecutionUnitsPerTransaction <- objectValue .: "maxExecutionUnitsPerTransaction" >>= executionUnitsFromJson
        maxExecutionUnitsPerBlock <- objectValue .: "maxExecutionUnitsPerBlock" >>= executionUnitsFromJson
        minFeeReferenceScripts <- objectValue .: "minFeeReferenceScripts" >>= minFeeReferenceScriptsFromJson
        poolVotingThresholds <- objectValue .: "stakePoolVotingThresholds" >>= poolVotingThresholdsFromJson
        dRepVotingThresholds <- objectValue .: "delegateRepresentativeVotingThresholds" >>= drepVotingThresholdsFromJson
        constitutionalCommitteeMinSize <- fromIntegral <$> (objectValue .: "constitutionalCommitteeMinSize" :: Parser Word64)
        constitutionalCommitteeMaxTermLength <- parseBounded objectValue "constitutionalCommitteeMaxTermLength"
        governanceActionLifetime <- parseBounded objectValue "governanceActionLifetime"
        governanceActionDepositValue <- objectValue .: "governanceActionDeposit"
        delegateRepresentativeDepositValue <- objectValue .: "delegateRepresentativeDeposit"
        delegateRepresentativeMaxIdleTime <- parseBounded objectValue "delegateRepresentativeMaxIdleTime"
        protocolVersion <- objectValue .: "version" >>= protocolVersionFromJson
        minUtxoDepositCoefficientValue <- fromIntegral <$> (objectValue .: "minUtxoDepositCoefficient" :: Parser Word64)
        minUtxoDepositConstantValue <- (objectValue .:? "minUtxoDepositConstant" :: Parser (Maybe Integer))
        plutusCostModels <- objectValue .: "plutusCostModels"

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
                        & ppCostModelsL .~ plutusCostModels
                }

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
            <$> (objectValue .: "memory" >>= parseBoundedRatio "scriptExecutionPrices.memory")
            <*> (objectValue .: "cpu" >>= parseBoundedRatio "scriptExecutionPrices.cpu")

executionUnitsFromJson :: Value -> Parser ExUnits
executionUnitsFromJson =
    withObject "ExecutionUnits" $ \objectValue ->
        ExUnits
            <$> (fromIntegral <$> (objectValue .: "memory" :: Parser Word64))
            <*> (fromIntegral <$> (objectValue .: "cpu" :: Parser Word64))

minFeeReferenceScriptsFromJson :: Value -> Parser NonNegativeInterval
minFeeReferenceScriptsFromJson =
    withObject "MinFeeReferenceScripts" $ \objectValue -> do
        range <- objectValue .: "range"
        base <- objectValue .: "base" >>= parseBoundedRatio "minFeeReferenceScripts.base"
        multiplier <- objectValue .: "multiplier" >>= ratioFromJson
        when (range /= (25600 :: Word64)) $
            fail ("minFeeReferenceScripts.range must be 25600 for the current Haskell ledger, but got " <> show range)
        when (multiplier /= (12 % 10)) $
            fail
                ( "minFeeReferenceScripts.multiplier must be 12/10 for the current Haskell ledger, but got "
                    <> toString (renderRatio multiplier)
                )
        pure base

poolVotingThresholdsFromJson :: Value -> Parser PoolVotingThresholds
poolVotingThresholdsFromJson =
    withObject "PoolVotingThresholds" $ \objectValue -> do
        noConfidence <- objectValue .: "noConfidence" >>= parseBoundedRatio "stakePoolVotingThresholds.noConfidence"
        constitutionalCommittee <- objectValue .: "constitutionalCommittee"
        hardForkInitiation <- objectValue .: "hardForkInitiation" >>= parseBoundedRatio "stakePoolVotingThresholds.hardForkInitiation"
        protocolParametersUpdate <- objectValue .: "protocolParametersUpdate"
        PoolVotingThresholds
            <$> pure noConfidence
            <*> (constitutionalCommittee .: "default" >>= parseBoundedRatio "stakePoolVotingThresholds.constitutionalCommittee.default")
            <*> (constitutionalCommittee .: "stateOfNoConfidence" >>= parseBoundedRatio "stakePoolVotingThresholds.constitutionalCommittee.stateOfNoConfidence")
            <*> pure hardForkInitiation
            <*> (protocolParametersUpdate .: "security" >>= parseBoundedRatio "stakePoolVotingThresholds.protocolParametersUpdate.security")

drepVotingThresholdsFromJson :: Value -> Parser DRepVotingThresholds
drepVotingThresholdsFromJson =
    withObject "DRepVotingThresholds" $ \objectValue -> do
        noConfidence <- objectValue .: "noConfidence" >>= parseBoundedRatio "delegateRepresentativeVotingThresholds.noConfidence"
        constitution <- objectValue .: "constitution" >>= parseBoundedRatio "delegateRepresentativeVotingThresholds.constitution"
        constitutionalCommittee <- objectValue .: "constitutionalCommittee"
        hardForkInitiation <- objectValue .: "hardForkInitiation" >>= parseBoundedRatio "delegateRepresentativeVotingThresholds.hardForkInitiation"
        protocolParametersUpdate <- objectValue .: "protocolParametersUpdate"
        treasuryWithdrawals <- objectValue .: "treasuryWithdrawals" >>= parseBoundedRatio "delegateRepresentativeVotingThresholds.treasuryWithdrawals"
        DRepVotingThresholds
            <$> pure noConfidence
            <*> (constitutionalCommittee .: "default" >>= parseBoundedRatio "delegateRepresentativeVotingThresholds.constitutionalCommittee.default")
            <*> (constitutionalCommittee .: "stateOfNoConfidence" >>= parseBoundedRatio "delegateRepresentativeVotingThresholds.constitutionalCommittee.stateOfNoConfidence")
            <*> pure constitution
            <*> pure hardForkInitiation
            <*> (protocolParametersUpdate .: "network" >>= parseBoundedRatio "delegateRepresentativeVotingThresholds.protocolParametersUpdate.network")
            <*> (protocolParametersUpdate .: "economic" >>= parseBoundedRatio "delegateRepresentativeVotingThresholds.protocolParametersUpdate.economic")
            <*> (protocolParametersUpdate .: "technical" >>= parseBoundedRatio "delegateRepresentativeVotingThresholds.protocolParametersUpdate.technical")
            <*> (protocolParametersUpdate .: "governance" >>= parseBoundedRatio "delegateRepresentativeVotingThresholds.protocolParametersUpdate.governance")
            <*> pure treasuryWithdrawals

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
