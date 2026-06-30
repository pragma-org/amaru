{-# LANGUAGE ExistentialQuantification #-}

module Data.KeyHash
    ( JsonKeyHash (..)
    , keyHashToText
    ) where

import Relude

import Cardano.Ledger.Hashes
    ( KeyHash
    )
import Data.Aeson
    ( ToJSON (..)
    , ToJSONKey (..)
    , Value (String)
    )
import Data.Aeson.Types
    ( toJSONKeyText
    )

data JsonKeyHash = forall keyRole. JsonKeyHash !(KeyHash keyRole)

instance Eq JsonKeyHash where
    left == right =
        jsonKeyHashToText left == jsonKeyHashToText right

instance Ord JsonKeyHash where
    compare left right =
        compare (jsonKeyHashToText left) (jsonKeyHashToText right)

instance ToJSON JsonKeyHash where
    toJSON =
        String . jsonKeyHashToText

instance ToJSONKey JsonKeyHash where
    toJSONKey =
        toJSONKeyText jsonKeyHashToText

keyHashToText :: KeyHash keyRole -> Text
keyHashToText keyHash =
    case toJSON keyHash of
        String text ->
            text
        _ ->
            error "KeyHash ToJSON did not produce a JSON string"

jsonKeyHashToText :: JsonKeyHash -> Text
jsonKeyHashToText (JsonKeyHash keyHash) =
    keyHashToText keyHash
