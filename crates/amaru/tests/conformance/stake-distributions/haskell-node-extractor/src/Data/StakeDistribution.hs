{-# LANGUAGE DeriveGeneric #-}

module Data.StakeDistribution
    ( StakeDistribution (..)
    , jsonConfig
    ) where

import Relude

import Data.Aeson
    ( KeyValue ((.=))
    , ToJSON (toEncoding, toJSON)
    , Value (Object)
    , pairs
    )
import Data.Aeson.Encode.Pretty
    ( Config (..)
    , keyOrder
    )
import Data.Coin
    ( JsonCoin
    )
import Data.PoolId
    ( JsonPoolId
    )
import Data.StakeDistribution.Account
    ( AccountsSummary
    )
import Data.StakeDistribution.DRep
    ( DRepsSummary
    )
import Data.StakeDistribution.Pool
    ( PoolSummary
    )
import Helpers.Json
    ( defaultConfig
    )

data StakeDistribution = StakeDistribution
    { epoch :: !Word64
    , treasury :: !JsonCoin
    , reserves :: !JsonCoin
    , activeStake :: !JsonCoin
    , drepsVotingStake :: !JsonCoin
    , poolsVotingStake :: !JsonCoin
    , accounts :: !AccountsSummary
    , dreps :: !DRepsSummary
    , pools :: !(Map JsonPoolId PoolSummary)
    }
    deriving (Generic)

instance ToJSON StakeDistribution where
    toJSON =
        Object . stakeDistributionFields
    toEncoding =
        pairs . stakeDistributionFields

stakeDistributionFields :: (KeyValue e kv, Monoid kv) => StakeDistribution -> kv
stakeDistributionFields distr = mempty
    <> "epoch" .= epoch distr
    <> "treasury" .= treasury distr
    <> "reserves" .= reserves distr
    <> "active_stake" .= activeStake distr
    <> "dreps_voting_stake" .= drepsVotingStake distr
    <> "pools_voting_stake" .= poolsVotingStake distr
    <> "accounts" .= accounts distr
    <> "dreps" .= dreps distr
    <> "pools" .= pools distr

jsonConfig :: Config
jsonConfig =
    defaultConfig
        { confCompare =
            keyOrder
                [ "epoch"
                , "treasury"
                , "reserves"
                , "active_stake"
                , "dreps_voting_stake"
                , "pools_voting_stake"
                , "accounts"
                , "dreps"
                , "pools"
                ]
                <> compare
        }
