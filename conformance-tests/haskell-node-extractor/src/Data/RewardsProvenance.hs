{-# LANGUAGE DataKinds #-}
{-# LANGUAGE DeriveGeneric #-}

module Data.RewardsProvenance
    ( RewardsProvenance (..)
    ) where

import Relude

import Data.Aeson
    ( KeyValue ((.=))
    , ToJSON (toEncoding, toJSON)
    , Value (Object)
    , pairs
    )
import Data.Coin
    ( JsonCoin
    )
import Data.PoolId
    ( JsonPoolId
    )
import Data.PoolRewardsInfo
    ( PoolRewardsInfo
    )
import Data.Rational
    ( JsonRational
    )

data RewardsProvenance = RewardsProvenance
    { totalStake :: !JsonCoin
      -- ^ The maximum Lovelace supply ('maxLL') less the current value of the reserves.
    , activeStake :: !JsonCoin
      -- ^ The amount of Lovelace that is delegated during the given epoch.
    , fees :: !JsonCoin
      -- ^ Fees collected for those rewards.
    , incentives :: !JsonCoin
      -- ^ The maximum amount of Lovelace which can be removed from the reserves
      -- to be given out as rewards for the given epoch. a.k.a ΔR1
    , treasuryTax :: !JsonCoin
      -- ^ The amount of Lovelace taken from the treasury for the given epoch. a.k.a ΔT1
    , totalRewards :: !JsonCoin
      -- ^ The reward pot for the given epoch, equal to ΔR1 + fee pot
    , efficiency :: !JsonRational
      -- ^ The ratio of the number of blocks actually made versus the number
      -- of blocks that were expected. a.k.a. η (eta)
    , stakePools :: !(Map JsonPoolId PoolRewardsInfo)
      -- ^ Stake pools specific information needed to compute the rewards for its members.
    }
    deriving (Generic)

instance ToJSON RewardsProvenance where
    toJSON =
        Object . rewardsProvenanceFields

    toEncoding =
        pairs . rewardsProvenanceFields

rewardsProvenanceFields :: (KeyValue e kv, Monoid kv) => RewardsProvenance -> kv
rewardsProvenanceFields rp = mempty
    <> "total_stake" .= totalStake rp
    <> "active_stake" .= activeStake rp
    <> "fees" .= fees rp
    <> "incentives" .= incentives rp
    <> "treasury_tax" .= treasuryTax rp
    <> "total_rewards" .= totalRewards rp
    <> "efficiency" .= efficiency rp
    <> "stake_pools" .= stakePools rp
