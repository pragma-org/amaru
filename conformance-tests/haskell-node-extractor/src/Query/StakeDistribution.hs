{-# LANGUAGE DataKinds #-}

module Query.StakeDistribution
    ( queryStakeDistribution
    ) where

import Relude

import Cardano.Ledger.Api.State.Query
    ( StakeSnapshots (ssMarkTotal, ssStakeSnapshots)
    , queryChainAccountState
    , queryDRepDelegations
    , queryDRepStakeDistr
    , queryDRepState
    , queryPoolState
    , queryRegisteredDRepStakeDistr
    , querySPOStakeDistr
    , queryStakeSnapshots
    , qpsrStakePoolParams
    , ssMarkPool
    )
import Cardano.Ledger.BaseTypes
    ( BlocksMade (BlocksMade)
    , Network
    , NonZero (unNonZero)
    )
import Cardano.Ledger.Shelley.LedgerState
    ( NewEpochState
    , applyRUpd
    , completeRupd
    , esLStateL
    , lsCertStateL
    , nesBcur
    , nesEsL
    , nesRu
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
import Data.Maybe.Strict
    ( StrictMaybe
        ( SJust
        , SNothing
        )
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
        , activeStake = JsonCoin (unNonZero (ssMarkTotal stakeSnapshots))
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
        nesBcur newEpochState

    poolParameters =
        qpsrStakePoolParams (queryPoolState newEpochState Nothing network)

    stakePerPool =
        Map.map ssMarkPool (ssStakeSnapshots stakeSnapshots)

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
        epochStateForAccounts ^. esLStateL . lsCertStateL . certDStateL . accountsL . accountsMapL

    instantStake =
        newEpochState ^. instantStakeL . instantStakeCredentialsL

    dRepDelegatees =
        Map.fromList
            [ (credential, drep)
            | (drep, delegators) <- Map.toAscList (queryDRepDelegations newEpochState Set.empty)
            , credential <- Set.toAscList delegators
            ]

    epochStateForAccounts =
        case nesRu newEpochState of
            SNothing ->
                newEpochState ^. nesEsL
            SJust pulsingRewardUpdate ->
                applyRUpd (completeRewardUpdate pulsingRewardUpdate) (newEpochState ^. nesEsL)

    completeRewardUpdate pulsingRewardUpdate =
        fst $
            runIdentity $
                runReaderT
                    (completeRupd pulsingRewardUpdate)
                    (error "completeRupd unexpectedly forced Globals while building a stake distribution")
