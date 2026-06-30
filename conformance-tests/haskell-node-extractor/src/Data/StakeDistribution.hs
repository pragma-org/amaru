{-# LANGUAGE DeriveGeneric #-}

module Data.StakeDistribution
    ( StakeDistribution (..)
    ) where

import Relude

import Data.Aeson
    ( ToJSON (toJSON)
    , genericToJSON
    )
import Data.Coin
    ( JsonCoin
    )
import Helpers.Json
    ( snakeCaseOptions
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
    , votingStake :: !JsonCoin
    , accounts :: !AccountsSummary
    , pools :: !(Map JsonPoolId PoolSummary)
    , dreps :: !DRepsSummary
    }
    deriving (Generic)

instance ToJSON StakeDistribution where
    toJSON =
        genericToJSON snakeCaseOptions
