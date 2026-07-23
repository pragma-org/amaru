module Data.Coin
    ( JsonCoin (..)
    ) where

import Cardano.Ledger.Coin
    ( Coin (unCoin)
    )
import Data.Aeson
    ( ToJSON (toJSON)
    )

newtype JsonCoin = JsonCoin
    { unJsonCoin :: Coin
    }

instance ToJSON JsonCoin where
    toJSON (JsonCoin coin) =
        toJSON (unCoin coin)
