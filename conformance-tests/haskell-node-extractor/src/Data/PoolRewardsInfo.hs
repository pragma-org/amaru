{-# LANGUAGE DataKinds #-}
{-# LANGUAGE DuplicateRecordFields #-}
{-# LANGUAGE NamedFieldPuns #-}

module Data.PoolRewardsInfo
    ( PoolRewardsInfo (..)
    ) where

import Relude

import Cardano.Ledger.Coin
    ( Coin
    )
import Data.Aeson
    ( ToJSON (toJSON)
    , object
    , (.=)
    )
import Data.Coin
    ( JsonCoin (JsonCoin)
    )
import Data.PoolDelegator
    ( PoolDelegator
    )
import Data.Rational
    ( JsonRational (JsonRational)
    )

data PoolRewardsInfo = PoolRewardsInfo
    { relativeStake :: !Rational
      -- ^ The stake pool's stake divided by the total stake
    , blocksMade :: !Natural
      -- ^ The number of blocks the stake pool produced in the previous epoch
    , totalRewards :: !Coin
      -- ^ The maximum rewards available for the entire pool
    , leaderReward :: !Coin
      -- ^ The leader/pool owner reward
    , delegators :: ![PoolDelegator]
      -- ^ A map of all its delegators, and their respective stake.
    }

instance ToJSON PoolRewardsInfo where
    toJSON PoolRewardsInfo{relativeStake, blocksMade, totalRewards, leaderReward, delegators} =
        object
            [ "relative_stake" .= JsonRational relativeStake
            , "blocks_made" .= blocksMade
            , "total_rewards" .= JsonCoin totalRewards
            , "leader_reward" .= JsonCoin leaderReward
            , "delegators" .= delegators
            ]
