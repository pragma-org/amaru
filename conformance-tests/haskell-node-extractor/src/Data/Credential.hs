module Data.Credential
    ( JsonCredential (..)
    , credentialPairs
    ) where

import Relude

import Cardano.Ledger.Credential
    ( Credential (KeyHashObj, ScriptHashObj)
    )
import Data.Aeson
    ( ToJSON (toJSON)
    , object
    , (.=)
    )
import Data.Aeson.Types
    ( Pair
    )

data JsonCredential keyRole = JsonCredential !(Credential keyRole)

instance ToJSON (JsonCredential keyRole) where
    toJSON (JsonCredential credential) =
        object (credentialPairs credential)

credentialPairs :: Credential keyRole -> [Pair]
credentialPairs = \case
    KeyHashObj keyHash ->
        [ "from" .= ("key" :: Text)
        , "hash" .= keyHash
        ]
    ScriptHashObj scriptHash ->
        [ "from" .= ("script" :: Text)
        , "hash" .= scriptHash
        ]
