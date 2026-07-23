module Data.ScriptHash
    ( JsonScriptHash (..)
    , scriptHashToText
    ) where

import Relude

import Cardano.Ledger.Hashes
    ( ScriptHash
    )
import Data.Aeson
    ( ToJSON (toJSON)
    , ToJSONKey (..)
    , Value (String)
    )
import Data.Aeson.Types
    ( toJSONKeyText
    )

newtype JsonScriptHash = JsonScriptHash
    { unJsonScriptHash :: ScriptHash
    }
    deriving (Eq, Ord)

instance ToJSON JsonScriptHash where
    toJSON =
        toJSON . scriptHashToText . unJsonScriptHash

instance ToJSONKey JsonScriptHash where
    toJSONKey =
        toJSONKeyText (scriptHashToText . unJsonScriptHash)

scriptHashToText :: ScriptHash -> Text
scriptHashToText scriptHash =
    case toJSON scriptHash of
        String text ->
            text
        _ ->
            error "ScriptHash ToJSON did not produce a JSON string"
