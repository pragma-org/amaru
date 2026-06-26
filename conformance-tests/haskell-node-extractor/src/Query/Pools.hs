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
    , stakePoolStateToStakePoolParams
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

import qualified Data.Map.Strict as Map

queryPools :: Network -> NewEpochState ConwayEra -> Pools
queryPools network newEpochState =
    Pools $ Map.mapWithKey (\poolId sps -> stakePoolStateToStakePoolParams network poolId sps)
          (certState ^. certPStateL ^. psStakePoolsL)
  where
    EpochState{esLState = LedgerState{lsCertState = certState}} =
        nesEs newEpochState

poolsOutputPath :: Word64 -> FilePath
poolsOutputPath epochNumber =
    "pools/" <> toString (show epochNumber :: Text) <> ".json"
