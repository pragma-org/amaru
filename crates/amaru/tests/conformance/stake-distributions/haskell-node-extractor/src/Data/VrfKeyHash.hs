{-# LANGUAGE ExistentialQuantification #-}

module Data.VrfKeyHash
    ( JsonVrfKeyHash (..)
    , vrfKeyHashToText
    ) where

import Relude

import Cardano.Ledger.Hashes
    ( VRFVerKeyHash
    )
import Data.Aeson
    ( ToJSON (..)
    , ToJSONKey (..)
    , Value (String)
    )
import Data.Aeson.Types
    ( toJSONKeyText
    )

data JsonVrfKeyHash = forall keyRole. JsonVrfKeyHash !(VRFVerKeyHash keyRole)

instance Eq JsonVrfKeyHash where
    left == right =
        jsonVrfKeyHashToText left == jsonVrfKeyHashToText right

instance Ord JsonVrfKeyHash where
    compare left right =
        compare (jsonVrfKeyHashToText left) (jsonVrfKeyHashToText right)

instance ToJSON JsonVrfKeyHash where
    toJSON =
        String . jsonVrfKeyHashToText

instance ToJSONKey JsonVrfKeyHash where
    toJSONKey =
        toJSONKeyText jsonVrfKeyHashToText

vrfKeyHashToText :: VRFVerKeyHash keyRole -> Text
vrfKeyHashToText vrfKeyHash =
    case toJSON vrfKeyHash of
        String text ->
            text
        _ ->
            error "VRFVerKeyHash ToJSON did not produce a JSON string"

jsonVrfKeyHashToText :: JsonVrfKeyHash -> Text
jsonVrfKeyHashToText (JsonVrfKeyHash vrfKeyHash) =
    vrfKeyHashToText vrfKeyHash
