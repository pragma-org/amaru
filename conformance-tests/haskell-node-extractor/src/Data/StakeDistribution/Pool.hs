{-# LANGUAGE DataKinds #-}
{-# LANGUAGE DeriveGeneric #-}
{-# LANGUAGE NamedFieldPuns #-}

module Data.StakeDistribution.Pool
    ( PoolSummary (..)
    , mkPoolSummaries
    ) where

import Relude

import Cardano.Ledger.BaseTypes
    ( BoundedRational (unboundRational)
    )
import Cardano.Ledger.Coin
    ( Coin
    )
import Cardano.Ledger.Hashes
    ( KeyHash
    )
import Cardano.Ledger.Keys
    ( KeyRole (..)
    )
import Cardano.Ledger.State
    ( StakePoolParams
        ( StakePoolParams
        , sppAccountAddress
        , sppCost
        , sppMargin
        , sppMetadata
        , sppOwners
        , sppPledge
        , sppRelays
        , sppVrf
        )
    )
import Data.Aeson
    ( KeyValue ((.=))
    , ToJSON (toEncoding, toJSON)
    , Value (Object)
    , pairs
    )
import Data.Coin
    ( JsonCoin (JsonCoin)
    )
import Data.KeyHash
    ( JsonKeyHash (JsonKeyHash)
    )
import Data.Metadata
    ( Metadata
    , metadataFromPoolMetadata
    )
import Data.Maybe.Strict
    ( strictMaybeToMaybe
    )
import Data.PoolId
    ( JsonPoolId (JsonPoolId)
    )
import Data.PoolRelay
    ( JsonPoolRelay (JsonPoolRelay)
    )
import Data.Rational
    ( JsonRational (JsonRational)
    )
import Data.RewardAddress
    ( JsonRewardAddress (JsonRewardAddress)
    )
import Data.VrfKeyHash
    ( JsonVrfKeyHash (JsonVrfKeyHash)
    )

import qualified Data.Map.Strict as Map
import qualified Data.Set as Set

data PoolSummary = PoolSummary
    { blocksCount :: !Word64
    , stake :: !JsonCoin
    , votingStake :: !JsonCoin
    , vrfKeyHash :: !JsonVrfKeyHash
    , pledge :: !JsonCoin
    , cost :: !JsonCoin
    , margin :: !JsonRational
    , rewardAddress :: !JsonRewardAddress
    , owners :: ![JsonKeyHash]
    , relays :: ![JsonPoolRelay]
    , metadata :: !(Maybe Metadata)
    }
    deriving (Generic)

instance ToJSON PoolSummary where
    toJSON =
        Object . poolSummaryFields

    toEncoding =
        pairs . poolSummaryFields

poolSummaryFields :: (KeyValue e kv, Monoid kv) => PoolSummary -> kv
poolSummaryFields pool = mempty
    <> "blocks_count" .= blocksCount pool
    <> "stake" .= stake pool
    <> "voting_stake" .= votingStake pool
    <> "vrf_key_hash" .= vrfKeyHash pool
    <> "pledge" .= pledge pool
    <> "cost" .= cost pool
    <> "margin" .= margin pool
    <> "reward_address" .= rewardAddress pool
    <> "owners" .= owners pool
    <> "relays" .= relays pool
    <> "metadata" .= metadata pool

mkPoolSummaries
    :: Map.Map (KeyHash StakePool) Coin
    -> Map.Map (KeyHash StakePool) Coin
    -> Map.Map (KeyHash StakePool) Natural
    -> Map.Map (KeyHash StakePool) StakePoolParams
    -> Map.Map JsonPoolId PoolSummary
mkPoolSummaries stakePerPool votingStakePerPool blocksPerPool poolParameters =
    Map.fromList
        [ (JsonPoolId poolId, mkPoolSummary poolId poolParameters')
        | (poolId, poolParameters') <- Map.toAscList poolParameters
        ]
  where
    mkPoolSummary poolId StakePoolParams{sppVrf, sppPledge, sppCost, sppMargin, sppAccountAddress, sppOwners, sppRelays, sppMetadata} =
        PoolSummary
            { blocksCount = fromIntegral (Map.findWithDefault 0 poolId blocksPerPool)
            , stake = JsonCoin (Map.findWithDefault mempty poolId stakePerPool)
            , votingStake = JsonCoin (Map.findWithDefault mempty poolId votingStakePerPool)
            , vrfKeyHash = JsonVrfKeyHash sppVrf
            , pledge = JsonCoin sppPledge
            , cost = JsonCoin sppCost
            , margin = JsonRational (unboundRational sppMargin)
            , rewardAddress = JsonRewardAddress sppAccountAddress
            , owners = fmap JsonKeyHash (Set.toAscList sppOwners)
            , relays = fmap JsonPoolRelay (toList sppRelays)
            , metadata = strictMaybeToMaybe sppMetadata <&> metadataFromPoolMetadata
            }
