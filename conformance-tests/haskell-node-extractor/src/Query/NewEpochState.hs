module Query.NewEpochState
    ( newEpochStateOutputPath
    , queryNewEpochState
    ) where

import Relude

import Cardano.Ledger.Shelley.LedgerState
    ( NewEpochState
    )
import Ouroboros.Consensus.Cardano.Block
    ( ConwayEra
    )

queryNewEpochState :: NewEpochState ConwayEra -> NewEpochState ConwayEra
queryNewEpochState =
    identity

newEpochStateOutputPath :: Word64 -> FilePath
newEpochStateOutputPath epochNumber =
    "data/newEpochState/" <> toString (show epochNumber :: Text) <> ".cbor"
