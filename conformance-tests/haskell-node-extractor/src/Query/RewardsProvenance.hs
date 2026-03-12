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
    ( KeyRole
        ( StakePool
        , Staking
        )
    )
import Cardano.Ledger.Shelley.LedgerState
    ( AccountState (AccountState, asReserves)
    , EpochState (EpochState, esAccountState, esSnapshots)
    , NewEpochState (nesBprev, nesEs)
    , prevPParamsEpochStateL
    )
import Cardano.Ledger.Shelley.Rewards
    ( LeaderOnlyReward (lRewardAmount)
    , StakeShare (unStakeShare)
    , mkPoolRewardInfo
    )
import Cardano.Ledger.State
    ( SnapShot (SnapShot)
    , SnapShots (ssFee, ssStakeGo)
    , Stake (unStake)
    , sumAllStake
    , sumStakePerPool
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
import Data.PoolRewardsInfo
    ( PoolRewardsInfo (..)
    )
import Data.Ratio
    ( (%)
    )
import Data.RewardsProvenance
    ( RewardsProvenance (..)
    )
import Data.VMap
    ( VMap
    )
import Genesis
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
    "data/rewardsProvenance/" <> toString (show epochNumber :: Text) <> ".json"

queryRewardsProvenance :: Genesis -> NewEpochState ConwayEra -> RewardsProvenance
queryRewardsProvenance Genesis{epochSize, maxSupply, activeSlotCoeff} newEpochState =
    RewardsProvenance
        { activeStake
        , efficiency
        , fees
        , incentives
        , stakePools
        , totalRewards = Coin rewardPot
        , totalStake
        , treasuryTax = Coin treasuryTax
        }
  where
    activeStake = sumAllStake stake
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
        VMap.toMap poolParams
            & fmap mkPoolRewardsInfo
            & Map.mapWithKey
                (toPoolRewardsInfo (delegatorsByPool stake delegations))
    Coin rewardPot =
        fees <> incentives
    totalStake = circulation epochState maxSupply
    treasuryTax =
        floor (protocolTau previousProtocolParameters * fromIntegral rewardPot)

    epochState = nesEs newEpochState
    blocks = nesBprev newEpochState
    EpochState{esAccountState, esSnapshots} = epochState
    AccountState{asReserves = Coin reserves} = esAccountState
    SnapShot stake delegations poolParams = ssStakeGo esSnapshots
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
    stakePerPool =
        sumStakePerPool delegations stake
    mkPoolRewardsInfo =
        mkPoolRewardInfo
            previousProtocolParameters
            availableRewards
            blocks
            (fromIntegral blocksCount)
            stake
            delegations
            stakePerPool
            totalStake
            activeStake

circulation :: EpochState ConwayEra -> Coin -> Coin
circulation EpochState{esAccountState = AccountState{asReserves}} supply =
    supply <-> asReserves

protocolRho :: PParams ConwayEra -> Rational
protocolRho (PParams protocolParameters) =
    unboundRational (PParams protocolParameters ^. ppRhoL)

protocolTau :: PParams ConwayEra -> Rational
protocolTau (PParams protocolParameters) =
    unboundRational (PParams protocolParameters ^. ppTauL)

delegatorsByPool
    :: Stake
    -> VMap VMap.VB VMap.VB (Credential 'Staking) (KeyHash 'StakePool)
    -> Map.Map (KeyHash 'StakePool) (Map.Map (Credential 'Staking) Coin)
delegatorsByPool stake delegations =
    VMap.foldlWithKey
        (flipFold insertDelegator)
        mempty
        delegations
  where
    insertDelegator credential poolId =
        Map.insertWith
            (<>)
            poolId
            (Map.singleton credential (delegatorStake credential))
    delegatorStake credential =
        maybe
            mempty
            (word64ToCoin . unCompactCoin)
            (VMap.lookup credential (unStake stake))

toPoolRewardsInfo
    :: Map.Map (KeyHash 'StakePool) (Map.Map (Credential 'Staking) Coin)
    -> KeyHash 'StakePool
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
    :: KeyHash 'StakePool
    -> Map.Map (KeyHash 'StakePool) (Map.Map (Credential 'Staking) Coin)
    -> [PoolDelegator]
poolDelegators poolId delegators =
    [ PoolDelegator{credential, stake}
    | (credential, stake) <- Map.toAscList (Map.findWithDefault mempty poolId delegators)
    ]

flipFold :: (k -> v -> a -> a) -> (a -> k -> v -> a)
flipFold f a k v =
    f k v a
