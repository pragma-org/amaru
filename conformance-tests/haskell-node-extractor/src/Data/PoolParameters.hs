{-# LANGUAGE DataKinds #-}
{-# LANGUAGE NamedFieldPuns #-}

module Data.PoolParameters
    ( JsonPoolParameters (..)
    ) where

import Relude

import Cardano.Ledger.Address
    ( AccountAddress (..)
    , AccountId
    )
import Cardano.Ledger.BaseTypes
    ( BoundedRational (unboundRational)
    , Network (Testnet)
    )
import Cardano.Ledger.Hashes
    ( KeyHash
    , VRFVerKeyHash (unVRFVerKeyHash)
    )
import Cardano.Ledger.Keys
    ( KeyRole (..)
    )
import Cardano.Ledger.State
    ( PoolMetadata
    , StakePoolState
        ( StakePoolState
        , spsCost
        , spsMargin
        , spsMetadata
        , spsOwners
        , spsRelays
        , spsPledge
        , spsVrf
        , spsAccountId
        )
    )
import Data.Aeson
    ( ToJSON (toJSON)
    , object
    , (.=)
    )
import Data.Aeson.Types
    ( Pair
    )
import Data.Coin
    ( JsonCoin (JsonCoin)
    )
import Data.Maybe.Strict
    ( StrictMaybe
        ( SJust
        , SNothing
        )
    )
import Data.PoolId
    ( JsonPoolId (JsonPoolId)
    )
import Data.PoolMetadata
    ( JsonPoolMetadata (JsonPoolMetadata)
    )
import Data.PoolRelay
    ( JsonPoolRelay (JsonPoolRelay)
    )
import Data.Rational
    ( JsonRational (JsonRational)
    )
import Data.RewardAccount
    ( JsonRewardAccount (JsonRewardAccount)
    )

import qualified Data.Set as Set

newtype JsonPoolParameters = JsonPoolParameters
    { unJsonPoolParameters :: (KeyHash StakePool, StakePoolState)
    }

instance ToJSON JsonPoolParameters where
    toJSON (JsonPoolParameters (poolId, StakePoolState{spsVrf, spsPledge, spsCost, spsMargin, spsAccountId, spsOwners, spsRelays, spsMetadata})) =
        object $
            [ "id" .= JsonPoolId poolId
            , "vrf" .= unVRFVerKeyHash spsVrf
            , "pledge" .= JsonCoin spsPledge
            , "cost" .= JsonCoin spsCost
            , "margin" .= JsonRational (unboundRational spsMargin)
            , "reward_account" .= JsonRewardAccount (AccountAddress Testnet spsAccountId)
            , "owners" .= Set.toAscList spsOwners
            , "relays" .= fmap JsonPoolRelay (toList spsRelays)
            ]
                <> metadataPair spsMetadata

metadataPair :: StrictMaybe PoolMetadata -> [Pair]
metadataPair = \case
    SNothing ->
        []
    SJust metadata ->
        [ "metadata" .= JsonPoolMetadata metadata
        ]
