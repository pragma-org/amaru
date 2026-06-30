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
    , defConfig
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

data StakeDistribution = StakeDistribution
    { epoch :: !Word64
    , treasury :: !JsonCoin
    , reserves :: !JsonCoin
    , activeStake :: !JsonCoin
    , poolsVotingStake :: !JsonCoin
    , drepsVotingStake :: !JsonCoin
    , accounts :: !AccountsSummary
    , pools :: !(Map JsonPoolId PoolSummary)
    , dreps :: !DRepsSummary
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
    <> "pools_voting_stake" .= poolsVotingStake distr
    <> "dreps_voting_stake" .= drepsVotingStake distr
    <> "accounts" .= accounts distr
    <> "pools" .= pools distr
    <> "dreps" .= dreps distr

jsonConfig :: Config
jsonConfig =
    defConfig
        { confCompare =
            keyOrder
                [ "epoch"
                , "treasury"
                , "reserves"
                , "active_stake"
                , "pools_voting_stake"
                , "dreps_voting_stake"
                , "accounts"
                , "pools"
                , "dreps"
                ]
                <> compare
        }
