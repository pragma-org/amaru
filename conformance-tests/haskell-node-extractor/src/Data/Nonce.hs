module Data.Nonce
    ( JsonNonce (..)
    ) where

import Relude

import Cardano.Ledger.BaseTypes
    ( Nonce (NeutralNonce, Nonce)
    )
import Data.Aeson
    ( ToJSON (toJSON)
    )

newtype JsonNonce = JsonNonce
    { unJsonNonce :: Nonce
    }

instance ToJSON JsonNonce where
    toJSON (JsonNonce nonce) = case nonce of
        NeutralNonce ->
            toJSON ("neutral" :: Text)
        Nonce hashValue ->
            toJSON hashValue
