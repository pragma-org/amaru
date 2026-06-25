{-# LANGUAGE DuplicateRecordFields #-}
{-# LANGUAGE NamedFieldPuns #-}

module Data.PoolMetadata
    ( JsonPoolMetadata (..)
    ) where

import Relude
    ()

import Cardano.Ledger.BaseTypes
    ( urlToText
    )
import Cardano.Ledger.State
    ( PoolMetadata
    )
import Data.Aeson
    ( ToJSON (toJSON)
    )

newtype JsonPoolMetadata = JsonPoolMetadata
    { unJsonPoolMetadata :: PoolMetadata
    }

instance ToJSON JsonPoolMetadata where
    toJSON (JsonPoolMetadata poolMetadata) = toJSON poolMetadata
