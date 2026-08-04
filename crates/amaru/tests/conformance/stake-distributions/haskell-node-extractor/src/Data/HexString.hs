module Data.HexString
    ( JsonHexString (..)
    , bytesToHexText
    ) where

import Relude

import Data.Aeson
    ( ToJSON (toJSON)
    )
import Data.Base16.Types
    ( extractBase16
    )
import Data.ByteString.Base16
    ( encodeBase16
    )

newtype JsonHexString = JsonHexString
    { unJsonHexString :: ByteString
    }

instance ToJSON JsonHexString where
    toJSON =
        toJSON . bytesToHexText . unJsonHexString

bytesToHexText :: ByteString -> Text
bytesToHexText =
    extractBase16 . encodeBase16
