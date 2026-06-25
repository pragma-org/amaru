{-# LANGUAGE NamedFieldPuns #-}

module Data.PoolParameters
    ( JsonPoolParameters (..)
    ) where

import Relude

import Cardano.Ledger.BaseTypes
    ( BoundedRational (unboundRational)
    )
import Cardano.Ledger.Hashes
    ( VRFVerKeyHash (unVRFVerKeyHash)
    )
import Cardano.Ledger.State
    ( PoolMetadata
    , StakePoolParams
        ( StakePoolParams
        , sppCost
        , sppId
        , sppMargin
        , sppMetadata
        , sppOwners
        , sppPledge
        , sppRelays
        , sppAccountAddress
        , sppVrf
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
    { unJsonPoolParameters :: StakePoolParams
    }

instance ToJSON JsonPoolParameters where
    toJSON (JsonPoolParameters StakePoolParams{sppId, sppVrf, sppPledge, sppCost, sppMargin, sppAccountAddress, sppOwners, sppRelays, sppMetadata}) =
        object $
            [ "id" .= JsonPoolId sppId
            , "vrf" .= unVRFVerKeyHash sppVrf
            , "pledge" .= JsonCoin sppPledge
            , "cost" .= JsonCoin sppCost
            , "margin" .= JsonRational (unboundRational sppMargin)
            , "reward_account" .= JsonRewardAccount sppAccountAddress
            , "owners" .= Set.toAscList sppOwners
            , "relays" .= fmap JsonPoolRelay (toList sppRelays)
            ]
                <> metadataPair sppMetadata

metadataPair :: StrictMaybe PoolMetadata -> [Pair]
metadataPair = \case
    SNothing ->
        []
    SJust metadata ->
        [ "metadata" .= JsonPoolMetadata metadata
        ]
