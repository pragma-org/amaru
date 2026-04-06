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
import Cardano.Ledger.PoolParams
    ( PoolMetadata
        ( PoolMetadata
        , pmHash
        , pmUrl
        )
    )
import Data.Aeson
    ( ToJSON (toJSON)
    , object
    , (.=)
    )
import Data.HexString
    ( JsonHexString (JsonHexString)
    )

newtype JsonPoolMetadata = JsonPoolMetadata
    { unJsonPoolMetadata :: PoolMetadata
    }

instance ToJSON JsonPoolMetadata where
    toJSON (JsonPoolMetadata PoolMetadata{pmUrl, pmHash}) =
        object
            [ "url" .= urlToText pmUrl
            , "hash" .= JsonHexString pmHash
            ]
