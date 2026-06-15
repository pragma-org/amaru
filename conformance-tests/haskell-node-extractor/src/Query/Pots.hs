module Query.Pots
    ( Pots (..)
    , potsOutputPath
    , queryPots
    ) where

import Relude

import Cardano.Ledger.Shelley.LedgerState
    ( NewEpochState
    )
import Cardano.Ledger.State
    ( ChainAccountState (casReserves, casTreasury)
    , chainAccountStateG
    )
import Lens.Micro
    ( (^.)
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
    { unPots :: ChainAccountState
    }

instance ToJSON Pots where
    toJSON (Pots chainAccountState) =
        object
            [ "treasury" .= JsonCoin (casTreasury chainAccountState)
            , "reserves" .= JsonCoin (casReserves chainAccountState)
            ]

queryPots :: NewEpochState era -> Pots
queryPots newEpochState =
    Pots (newEpochState ^. chainAccountStateG)

potsOutputPath :: Word64 -> FilePath
potsOutputPath epochNumber =
    "data/pots/" <> toString (show epochNumber :: Text) <> ".json"
