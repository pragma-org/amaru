{-# LANGUAGE DataKinds #-}

module Data.Pools
    ( Pools (..)
    ) where

import Relude
    ()

import Cardano.Ledger.Hashes
    ( KeyHash
    )
import Cardano.Ledger.Keys
    ( KeyRole (StakePool)
    )
import Cardano.Ledger.PoolParams
    ( PoolParams
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
import Data.PoolId
    ( poolIdText
    )
import Data.PoolParameters
    ( JsonPoolParameters (JsonPoolParameters)
    )

import qualified Data.Map.Strict as Map

newtype Pools = Pools
    { unPools :: Map.Map (KeyHash 'StakePool) PoolParams
    }

instance ToJSON Pools where
    toJSON (Pools pools) =
        object (poolPairs pools)

poolPairs :: Map.Map (KeyHash 'StakePool) PoolParams -> [Pair]
poolPairs pools =
    [ fromText (poolIdText poolId) .= JsonPoolParameters poolParameters
    | (poolId, poolParameters) <- Map.toAscList pools
    ]
