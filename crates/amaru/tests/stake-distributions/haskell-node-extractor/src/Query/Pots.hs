module Query.Pots
    ( Pots (..)
    , potsOutputPath
    , queryPots
    ) where

import Relude

import Cardano.Ledger.Shelley.LedgerState
    ( EpochState (esChainAccountState)
    , NewEpochState (nesEs)
    )
import Cardano.Ledger.State
    ( ChainAccountState (casTreasury, casReserves)
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
queryPots =
    Pots . esChainAccountState . nesEs

potsOutputPath :: Word64 -> FilePath
potsOutputPath epochNumber =
    "pots/" <> toString (show epochNumber :: Text) <> ".json"
