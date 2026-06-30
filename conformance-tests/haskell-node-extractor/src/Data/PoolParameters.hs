{-# LANGUAGE NamedFieldPuns #-}

module Data.PoolParameters
    ( JsonPoolParameters (..)
    ) where

import Relude

import Cardano.Ledger.BaseTypes
    ( BoundedRational (unboundRational)
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
import Data.KeyHash
    ( JsonKeyHash (JsonKeyHash)
    )
import Data.Maybe.Strict
    ( StrictMaybe
        ( SJust
        , SNothing
        )
    )
import Data.Metadata
    ( Metadata
    , metadataFromPoolMetadata
    )
import Data.PoolId
    ( JsonPoolId (JsonPoolId)
    )
import Data.PoolRelay
    ( JsonPoolRelay (JsonPoolRelay)
    )
import Data.Rational
    ( JsonRational (JsonRational)
    )
import Data.RewardAddress
    ( JsonRewardAddress (JsonRewardAddress)
    )
import Data.VrfKeyHash
    ( JsonVrfKeyHash (JsonVrfKeyHash)
    )

import qualified Data.Set as Set

newtype JsonPoolParameters = JsonPoolParameters
    { unJsonPoolParameters :: StakePoolParams
    }

instance ToJSON JsonPoolParameters where
    toJSON (JsonPoolParameters StakePoolParams{sppId, sppVrf, sppPledge, sppCost, sppMargin, sppAccountAddress, sppOwners, sppRelays, sppMetadata}) =
        object $
            [ "id" .= JsonPoolId sppId
            , "vrf" .= JsonVrfKeyHash sppVrf
            , "pledge" .= JsonCoin sppPledge
            , "cost" .= JsonCoin sppCost
            , "margin" .= JsonRational (unboundRational sppMargin)
            , "reward_address" .= JsonRewardAddress sppAccountAddress
            , "owners" .= fmap JsonKeyHash (Set.toAscList sppOwners)
            , "relays" .= fmap JsonPoolRelay (toList sppRelays)
            ]
                <> metadataPair sppMetadata

metadataPair :: StrictMaybe PoolMetadata -> [Pair]
metadataPair = \case
    SNothing ->
        []
    SJust metadata ->
        [ "metadata" .= (metadataFromPoolMetadata metadata :: Metadata)
        ]
