module Query.Pots
    ( Pots (..)
    , potsOutputPath
    , queryPots
    ) where

import Relude

import Cardano.Ledger.Alonzo.State
    ( AccountState (asReserves, asTreasury)
    )
import Cardano.Ledger.Api.State.Query
    ( queryAccountState
    )
import Cardano.Ledger.Shelley.LedgerState
    ( NewEpochState
    )
import Data.Aeson
    ( ToJSON (toJSON)
    , object
    , (.=)
    )
import Data.Coin
    ( JsonCoin (JsonCoin)
    )

newtype Pots = Pots
    { unPots :: AccountState
    }

instance ToJSON Pots where
    toJSON (Pots accountState) =
        object
            [ "treasury" .= JsonCoin (asTreasury accountState)
            , "reserves" .= JsonCoin (asReserves accountState)
            ]

queryPots :: NewEpochState era -> Pots
queryPots =
    Pots . queryAccountState

potsOutputPath :: Word64 -> FilePath
potsOutputPath epochNumber =
    "data/pots/" <> toString (show epochNumber :: Text) <> ".json"
