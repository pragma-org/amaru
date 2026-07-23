{-# LANGUAGE DataKinds #-}
{-# LANGUAGE DuplicateRecordFields #-}
{-# LANGUAGE NamedFieldPuns #-}

module Query.RewardsProvenance
    ( rewardsProvenanceOutputPath
    , queryRewardsProvenance
    ) where

import Relude

import Cardano.Ledger.BaseTypes
    ( BlocksMade (BlocksMade)
    , BoundedRational (unboundRational)
    , NonZero (..)
    , activeSlotVal
    )
import Cardano.Ledger.Coin
    ( Coin (Coin)
    , CompactForm (unCompactCoin)
    , rationalToCoinViaFloor
    , word64ToCoin
    )
import Cardano.Ledger.Core
    ( PParams (PParams)
    , ppRhoL
    , ppTauL
    )
import Cardano.Ledger.Credential
    ( Credential
    )
import Cardano.Ledger.Hashes
    ( KeyHash
    )
import Cardano.Ledger.Keys
    ( KeyRole (..)
    )
import Cardano.Ledger.Shelley.LedgerState
    ( EpochState (EpochState, esChainAccountState, esSnapshots)
    , NewEpochState (nesBprev, nesEs)
    , prevPParamsEpochStateL
    )
import Cardano.Ledger.State
    ( ActiveStake (..)
    , ChainAccountState (ChainAccountState, casReserves)
    , SnapShot (..)
    , SnapShots (ssFee, ssStakeGo)
    , StakeWithDelegation (..)
    )
import Cardano.Ledger.Shelley.Rewards
    ( LeaderOnlyReward (lRewardAmount)
    , StakeShare (unStakeShare)
    , mkPoolRewardInfo
    )
import Cardano.Ledger.Val
    ( (<->)
    )
import Cardano.Slotting.Slot
    ( EpochSize (unEpochSize)
    )
import Data.PoolDelegator
    ( PoolDelegator (..)
    )
import Data.Coin
    ( JsonCoin (JsonCoin)
    )
import Data.PoolId
    ( JsonPoolId (JsonPoolId)
    )
import Data.PoolRewardsInfo
    ( PoolRewardsInfo (..)
    )
import Data.Ratio
    ( (%)
    )
import Data.Rational
    ( JsonRational (JsonRational)
    )
import Data.RewardsProvenance
    ( RewardsProvenance (..)
    )
import Data.Genesis
    ( Genesis (Genesis, activeSlotCoeff, epochSize, maxSupply)
    )
import Lens.Micro
    ( (^.)
    )
import Ouroboros.Consensus.Cardano.Block
    ( ConwayEra
    )

import qualified Cardano.Ledger.Shelley.Rewards as Ledger
import qualified Data.Map.Strict as Map
import qualified Data.VMap as VMap


rewardsProvenanceOutputPath :: Word64 -> FilePath
rewardsProvenanceOutputPath epochNumber =
    "rewardsProvenance/" <> toString (show epochNumber :: Text) <> ".json"

queryRewardsProvenance :: Genesis -> NewEpochState ConwayEra -> RewardsProvenance
queryRewardsProvenance Genesis{epochSize, maxSupply, activeSlotCoeff} newEpochState =
    RewardsProvenance
        { activeStake = JsonCoin activeStake
        , efficiency = JsonRational efficiency
        , fees = JsonCoin fees
        , incentives = JsonCoin incentives
        , stakePools
        , totalRewards = JsonCoin (Coin rewardPot)
        , totalStake = JsonCoin totalStake
        , treasuryTax = JsonCoin (Coin treasuryTax)
        }
  where
    activeStake = unNonZero ssTotalActiveStake
    efficiency
        | expectedBlocks == 0 =
            1
        | otherwise =
            blocksCount % expectedBlocks
    fees = ssFee esSnapshots
    incentives = rationalToCoinViaFloor $
        min 1 efficiency
            * protocolRho previousProtocolParameters
            * fromIntegral reserves
    stakePools =
        VMap.toMap ssStakePoolsSnapShot
            & Map.mapWithKey mkPoolRewardsInfo'
            & Map.mapWithKey
                (toPoolRewardsInfo (delegatorsByPool ssActiveStake))
            & Map.mapKeysMonotonic JsonPoolId
    Coin rewardPot =
        fees <> incentives
    totalStake = circulation epochState maxSupply
    treasuryTax =
        floor (protocolTau previousProtocolParameters * fromIntegral rewardPot)

    epochState = nesEs newEpochState
    blocks = nesBprev newEpochState
    EpochState{esChainAccountState, esSnapshots} = epochState
    ChainAccountState{casReserves = Coin reserves} = esChainAccountState
    SnapShot{ssActiveStake, ssTotalActiveStake, ssStakePoolsSnapShot} = ssStakeGo esSnapshots
    previousProtocolParameters = epochState ^. prevPParamsEpochStateL
    blocksCount =
        fromIntegral $
            Map.foldr (+) 0 blockCounts
      where
        BlocksMade blockCounts =
            blocks
    expectedBlocks =
        floor $ unboundRational (activeSlotVal activeSlotCoeff) * fromIntegral (unEpochSize epochSize)
    availableRewards =
        Coin (rewardPot - treasuryTax)
    mkPoolRewardsInfo' poolId stakePoolSnapShot =
        mkPoolRewardInfo
            previousProtocolParameters
            availableRewards
            blocks
            (fromIntegral blocksCount)
            totalStake
            ssTotalActiveStake
            poolId
            stakePoolSnapShot

circulation :: EpochState ConwayEra -> Coin -> Coin
circulation EpochState{esChainAccountState = ChainAccountState{casReserves}} supply =
    supply <-> casReserves

protocolRho :: PParams ConwayEra -> Rational
protocolRho (PParams protocolParameters) =
    unboundRational (PParams protocolParameters ^. ppRhoL)

protocolTau :: PParams ConwayEra -> Rational
protocolTau (PParams protocolParameters) =
    unboundRational (PParams protocolParameters ^. ppTauL)

delegatorsByPool
    :: ActiveStake
    -> Map.Map (KeyHash StakePool) (Map.Map (Credential Staking) Coin)
delegatorsByPool (ActiveStake m) =
    VMap.foldlWithKey accum mempty m
  where
    accum acc cred StakeWithDelegation{swdStake, swdDelegation} =
        Map.insertWith
            (<>)
            swdDelegation
            (Map.singleton cred (word64ToCoin (unCompactCoin (unNonZero swdStake))))
            acc

toPoolRewardsInfo
    :: Map.Map (KeyHash StakePool) (Map.Map (Credential Staking) Coin)
    -> KeyHash StakePool
    -> Either Ledger.StakeShare Ledger.PoolRewardInfo
    -> PoolRewardsInfo
toPoolRewardsInfo delegators poolId = \case
    Left stakeShare ->
        PoolRewardsInfo
            { relativeStake = unStakeShare stakeShare
            , blocksMade = 0
            , totalRewards = mempty
            , leaderReward = mempty
            , delegators = poolDelegators poolId delegators
            }
    Right poolRewardsInfo ->
        PoolRewardsInfo
            { relativeStake = unStakeShare (Ledger.poolRelativeStake poolRewardsInfo)
            , blocksMade = Ledger.poolBlocks poolRewardsInfo
            , totalRewards = Ledger.poolPot poolRewardsInfo
            , leaderReward = lRewardAmount (Ledger.poolLeaderReward poolRewardsInfo)
            , delegators = poolDelegators poolId delegators
            }

poolDelegators
    :: KeyHash StakePool
    -> Map.Map (KeyHash StakePool) (Map.Map (Credential Staking) Coin)
    -> [PoolDelegator]
poolDelegators poolId delegators =
    [ PoolDelegator{credential, stake}
    | (credential, stake) <- Map.toAscList (Map.findWithDefault mempty poolId delegators)
    ]
