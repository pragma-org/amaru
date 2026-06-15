{-# LANGUAGE DataKinds #-}
{-# LANGUAGE DuplicateRecordFields #-}
{-# LANGUAGE NamedFieldPuns #-}

module Query.RewardsProvenance
    ( rewardsProvenanceOutputPath
    , queryRewardsProvenance
    ) where

import Relude

import Cardano.Ledger.Coin
    ( Coin (Coin)
    , rationalToCoinViaFloor
    )
import Cardano.Ledger.Compactible
    ( fromCompact
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
import Cardano.Ledger.State
    ( ActiveStake (..)
    , ChainAccountState (ChainAccountState, casReserves)
    , SnapShot (..)
    , SnapShots (ssFee, ssStakeGo)
    , StakeWithDelegation (..)
    , casReservesL
    , chainAccountStateG
    )
import Cardano.Ledger.BaseTypes
    ( BlocksMade (BlocksMade)
    , BoundedRational (unboundRational)
    , NonZero (..)
    , activeSlotVal
    )
import Cardano.Ledger.Shelley.LedgerState
    ( EpochState (EpochState, esSnapshots)
    , NewEpochState (nesBprev, nesEs)
    , prevPParamsEpochStateL
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
import Data.PoolRewardsInfo
    ( PoolRewardsInfo (..)
    )
import Data.Ratio
    ( (%)
    )
import Data.RewardsProvenance
    ( RewardsProvenance (..)
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
        { activeStake = unNonZero ssTotalActiveStake
        , efficiency
        , fees
        , incentives
        , stakePools
        , totalRewards = Coin rewardPot
        , totalStake
        , treasuryTax = Coin treasuryTax
        }
  where
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
            & Map.mapWithKey
                ( \poolId spss ->
                    toPoolRewardsInfo
                        (Map.findWithDefault Map.empty poolId poolDelegatorsMap)
                        (mkPoolRewardInfo
                            previousProtocolParameters
                            availableRewards
                            blocks
                            (fromIntegral blocksCount)
                            totalStake
                            ssTotalActiveStake
                            poolId
                            spss
                        )
                )
    Coin rewardPot =
        fees <> incentives
    totalStake = circulation epochState maxSupply
    treasuryTax =
        floor (protocolTau previousProtocolParameters * fromIntegral rewardPot)

    epochState = nesEs newEpochState
    blocks = nesBprev newEpochState
    EpochState{esSnapshots} = epochState
    ChainAccountState{casReserves = Coin reserves} = epochState ^. chainAccountStateG
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
    poolDelegatorsMap = delegatorsByPool ssActiveStake

circulation :: EpochState ConwayEra -> Coin -> Coin
circulation epochState supply =
    supply <-> (epochState ^. chainAccountStateG . casReservesL)

protocolRho :: PParams ConwayEra -> Rational
protocolRho (PParams protocolParameters) =
    unboundRational (PParams protocolParameters ^. ppRhoL)

protocolTau :: PParams ConwayEra -> Rational
protocolTau (PParams protocolParameters) =
    unboundRational (PParams protocolParameters ^. ppTauL)

delegatorsByPool
    :: ActiveStake
    -> Map.Map (KeyHash StakePool) (Map.Map (Credential Staking) Coin)
delegatorsByPool (ActiveStake vmap) =
    VMap.foldlWithKey accum Map.empty vmap
  where
    accum acc cred StakeWithDelegation{swdStake, swdDelegation} =
        Map.insertWith
            (<>)
            swdDelegation
            (Map.singleton cred (fromCompact (unNonZero swdStake)))
            acc

toPoolRewardsInfo
    :: Map.Map (Credential Staking) Coin
    -> Either Ledger.StakeShare Ledger.PoolRewardInfo
    -> PoolRewardsInfo
toPoolRewardsInfo delegatorsForPool = \case
    Left stakeShare ->
        PoolRewardsInfo
            { relativeStake = unStakeShare stakeShare
            , blocksMade = 0
            , totalRewards = mempty
            , leaderReward = mempty
            , delegators = mkDelegators delegatorsForPool
            }
    Right poolRewardsInfo ->
        PoolRewardsInfo
            { relativeStake = unStakeShare (Ledger.poolRelativeStake poolRewardsInfo)
            , blocksMade = Ledger.poolBlocks poolRewardsInfo
            , totalRewards = Ledger.poolPot poolRewardsInfo
            , leaderReward = lRewardAmount (Ledger.poolLeaderReward poolRewardsInfo)
            , delegators = mkDelegators delegatorsForPool
            }

mkDelegators :: Map.Map (Credential Staking) Coin -> [PoolDelegator]
mkDelegators m =
    [ PoolDelegator{credential, stake}
    | (credential, stake) <- Map.toAscList m
    ]
