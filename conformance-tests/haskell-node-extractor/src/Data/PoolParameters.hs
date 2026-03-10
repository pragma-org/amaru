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
import Cardano.Ledger.PoolParams
    ( PoolMetadata
    , PoolParams
        ( PoolParams
        , ppCost
        , ppId
        , ppMargin
        , ppMetadata
        , ppOwners
        , ppPledge
        , ppRelays
        , ppRewardAccount
        , ppVrf
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
    { unJsonPoolParameters :: PoolParams
    }

instance ToJSON JsonPoolParameters where
    toJSON (JsonPoolParameters PoolParams{ppId, ppVrf, ppPledge, ppCost, ppMargin, ppRewardAccount, ppOwners, ppRelays, ppMetadata}) =
        object $
            [ "id" .= JsonPoolId ppId
            , "vrf" .= unVRFVerKeyHash ppVrf
            , "pledge" .= JsonCoin ppPledge
            , "cost" .= JsonCoin ppCost
            , "margin" .= JsonRational (unboundRational ppMargin)
            , "reward_account" .= JsonRewardAccount ppRewardAccount
            , "owners" .= Set.toAscList ppOwners
            , "relays" .= fmap JsonPoolRelay (toList ppRelays)
            ]
                <> metadataPair ppMetadata

metadataPair :: StrictMaybe PoolMetadata -> [Pair]
metadataPair = \case
    SNothing ->
        []
    SJust metadata ->
        [ "metadata" .= JsonPoolMetadata metadata
        ]
