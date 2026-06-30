{-# LANGUAGE DataKinds #-}

module Query.StakeDistribution
    ( queryStakeDistribution
    ) where

import Relude

import Cardano.Ledger.Api.State.Query
    ( StakeSnapshots (ssGoTotal, ssStakeSnapshots)
    , queryChainAccountState
    , queryDRepDelegations
    , queryDRepStakeDistr
    , queryDRepState
    , queryPoolState
    , queryRegisteredDRepStakeDistr
    , querySPOStakeDistr
    , queryStakeSnapshots
    , qpsrStakePoolParams
    , ssGoPool
    )
import Cardano.Ledger.BaseTypes
    ( BlocksMade (BlocksMade)
    , Network
    , NonZero (unNonZero)
    )
import Cardano.Ledger.Shelley.LedgerState
    ( NewEpochState
    , esLStateL
    , lsCertStateL
    , nesBprev
    , nesEsL
    )
import Cardano.Ledger.State
    ( ChainAccountState (..)
    , CanSetAccounts (accountsL)
    , CanSetInstantStake (instantStakeL)
    , EraAccounts
        ( accountsMapL
        )
    , EraCertState (certDStateL)
    , EraStake (instantStakeCredentialsL)
    )
import Data.Coin
    ( JsonCoin (JsonCoin)
    )
import Data.StakeDistribution
    ( StakeDistribution (..)
    )
import Data.StakeDistribution.Account
    ( mkAccountSummary
    , mkAccountsSummary
    )
import Data.StakeDistribution.DRep
    ( mkDRepsSummary
    )
import Data.StakeDistribution.Pool
    ( mkPoolSummaries
    )
import Lens.Micro
    ( (^.)
    )
import Ouroboros.Consensus.Cardano.Block (ConwayEra)

import qualified Data.Map.Strict as Map
import qualified Data.Set as Set

queryStakeDistribution
    :: Network
    -> Word64
    -> NewEpochState ConwayEra
    -> StakeDistribution
queryStakeDistribution network epochNumber newEpochState =
    StakeDistribution
        { epoch = epochNumber
        , treasury = JsonCoin treasury
        , reserves = JsonCoin reserves
        , activeStake = JsonCoin (unNonZero (ssGoTotal stakeSnapshots))
        , poolsVotingStake = JsonCoin (fold votingStakePerPool)
        , drepsVotingStake = JsonCoin (fold dRepStakeDistribution)
        , accounts = mkAccountsSummary accountSummaries
        , pools = mkPoolSummaries stakePerPool votingStakePerPool blocksPerPool poolParameters
        , dreps = mkDRepsSummary registeredDRepStates registeredDRepStakeDistribution dRepStakeDistribution
        }
  where
    ChainAccountState{casReserves = reserves, casTreasury = treasury} =
        queryChainAccountState newEpochState

    stakeSnapshots =
        queryStakeSnapshots newEpochState Nothing

    BlocksMade blocksPerPool =
        nesBprev newEpochState

    poolParameters =
        qpsrStakePoolParams (queryPoolState newEpochState Nothing network)

    stakePerPool =
        Map.map ssGoPool (ssStakeSnapshots stakeSnapshots)

    votingStakePerPool =
        querySPOStakeDistr newEpochState Set.empty

    registeredDRepStates =
        queryDRepState newEpochState Set.empty

    dRepStakeDistribution =
        queryDRepStakeDistr newEpochState Set.empty

    registeredDRepStakeDistribution =
        queryRegisteredDRepStakeDistr newEpochState Set.empty

    accountSummaries =
        Map.mapWithKey (mkAccountSummary instantStake dRepDelegatees) accountsMap

    accountsMap =
        newEpochState ^. nesEsL . esLStateL . lsCertStateL . certDStateL . accountsL . accountsMapL

    instantStake =
        newEpochState ^. instantStakeL . instantStakeCredentialsL

    dRepDelegatees =
        Map.fromList
            [ (credential, drep)
            | (drep, delegators) <- Map.toAscList (queryDRepDelegations newEpochState Set.empty)
            , credential <- Set.toAscList delegators
            ]
