module Query.Pools
    ( poolsOutputPath
    , queryPools
    ) where

import Relude

import Cardano.Ledger.BaseTypes
    ( Network
    )
import Cardano.Ledger.State
    ( certPStateL
    , psStakePoolsL
    )
import Cardano.Ledger.Shelley.LedgerState
    ( EpochState (EpochState, esLState)
    , LedgerState (LedgerState, lsCertState)
    , NewEpochState (nesEs)
    )
import Data.Pools
    ( Pools (Pools)
    )
import Lens.Micro
    ( (^.)
    )
import Ouroboros.Consensus.Cardano.Block
    ( ConwayEra
    )

queryPools :: Network -> NewEpochState ConwayEra -> Pools
queryPools _network newEpochState =
    Pools (certState ^. certPStateL ^. psStakePoolsL)
  where
    EpochState{esLState = LedgerState{lsCertState = certState}} =
        nesEs newEpochState

poolsOutputPath :: Word64 -> FilePath
poolsOutputPath epochNumber =
    "data/pools/" <> toString (show epochNumber :: Text) <> ".json"
