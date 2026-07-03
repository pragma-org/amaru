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
    ( KeyRole (..)
    )
import Cardano.Ledger.State
    ( StakePoolParams
    )
import Data.Aeson
    ( ToJSON (toJSON)
    )
import Data.PoolId
    ( JsonPoolId (JsonPoolId)
    )
import Data.PoolParameters
    ( JsonPoolParameters (JsonPoolParameters)
    )

import qualified Data.Map.Strict as Map

newtype Pools = Pools
    { unPools :: Map.Map (KeyHash StakePool) StakePoolParams
    }

instance ToJSON Pools where
    toJSON (Pools pools) =
        toJSON
            ( Map.map JsonPoolParameters
                (Map.mapKeysMonotonic JsonPoolId pools)
                :: Map.Map JsonPoolId JsonPoolParameters
            )
