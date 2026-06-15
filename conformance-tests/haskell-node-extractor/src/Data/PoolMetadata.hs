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
        ( PoolMetadata
        , pmHash
        , pmUrl
        )
    )
import Data.Array.Byte
    ( ByteArray
    )
import Data.ByteString.Short
    ( fromShort
    )
import Data.MemPack.Buffer
    ( byteArrayToShortByteString
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
            , "hash" .= JsonHexString (fromShort (byteArrayToShortByteString pmHash))
            ]
