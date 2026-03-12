{-# LANGUAGE DataKinds #-}
{-# LANGUAGE DuplicateRecordFields #-}
{-# LANGUAGE NamedFieldPuns #-}

module Data.RewardsProvenance
    ( RewardsProvenance (..)
    ) where

import Relude

import Cardano.Ledger.Coin
    ( Coin
    )
import Cardano.Ledger.Hashes
    ( KeyHash
    )
import Cardano.Ledger.Keys
    ( KeyRole (StakePool)
    )
import Data.Aeson
    ( ToJSON (toJSON)
    , object
    , (.=)
    )
import Data.Aeson.Key
    ( fromText
    )
import Data.Aeson.Types
    ( Pair
    )
import Data.Coin
    ( JsonCoin (JsonCoin)
    )
import Data.PoolId
    ( poolIdText
    )
import Data.PoolRewardsInfo
    ( PoolRewardsInfo
    )
import Data.Rational
    ( JsonRational (JsonRational)
    )

import qualified Data.Map.Strict as Map

data RewardsProvenance = RewardsProvenance
    { totalStake :: !Coin
      -- ^ The maximum Lovelace supply ('maxLL') less the current value of the reserves.
    , activeStake :: !Coin
      -- ^ The amount of Lovelace that is delegated during the given epoch.
    , fees :: !Coin
      -- ^ Fees collected for those rewards.
    , incentives :: !Coin
      -- ^ The maximum amount of Lovelace which can be removed from the reserves
      -- to be given out as rewards for the given epoch. a.k.a ΔR1
    , treasuryTax :: !Coin
      -- ^ The amount of Lovelace taken from the treasury for the given epoch. a.k.a ΔT1
    , totalRewards :: !Coin
      -- ^ The reward pot for the given epoch, equal to ΔR1 + fee pot
    , efficiency :: !Rational
      -- ^ The ratio of the number of blocks actually made versus the number
      -- of blocks that were expected. a.k.a. η (eta)
    , stakePools :: !(Map.Map (KeyHash 'StakePool) PoolRewardsInfo)
      -- ^ Stake pools specific information needed to compute the rewards for its members.
    }

instance ToJSON RewardsProvenance where
    toJSON RewardsProvenance{totalStake, activeStake, fees, incentives, treasuryTax, totalRewards, efficiency, stakePools} =
        object
            [ "total_stake" .= JsonCoin totalStake
            , "active_stake" .= JsonCoin activeStake
            , "fees" .= JsonCoin fees
            , "incentives" .= JsonCoin incentives
            , "treasury_tax" .= JsonCoin treasuryTax
            , "total_rewards" .= JsonCoin totalRewards
            , "efficiency" .= JsonRational efficiency
            , "stake_pools" .= object (stakePoolPairs stakePools)
            ]

stakePoolPairs :: Map.Map (KeyHash 'StakePool) PoolRewardsInfo -> [Pair]
stakePoolPairs stakePools =
    [ fromText (poolIdText poolId) .= poolRewardsInfo
    | (poolId, poolRewardsInfo) <- Map.toAscList stakePools
    ]
