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
    -> NewEpochState ConwayEra
    -> StakeDistribution
queryStakeDistribution network epochNumber targetEpochState nextEpochState =
    StakeDistribution
        { epoch = epochNumber
        , treasury = JsonCoin treasury
        , reserves = JsonCoin reserves
        , activeStake = JsonCoin (unNonZero (ssMarkTotal stakeSnapshots))
        , poolsVotingStake = JsonCoin (fold votingStakePerPool)
        , drepsVotingStake = JsonCoin (fold dRepStakeDistribution)
        , accounts = mkAccountsSummary accountSummaries
        , pools = mkPoolSummaries stakePerPool votingStakePerPool blocksPerPool poolParameters
        , dreps = mkDRepsSummary registeredDRepStates dRepStakeDistribution
        }
  where
    ChainAccountState{casReserves = reserves, casTreasury = treasury} =
        queryChainAccountState targetEpochState

    stakeSnapshots =
        queryStakeSnapshots nextEpochState Nothing

    BlocksMade blocksPerPool =
        nesBcur targetEpochState

    poolParameters =
        qpsrStakePoolParams (queryPoolState targetEpochState Nothing network)

    stakePerPool =
        Map.map ssMarkPool (ssStakeSnapshots stakeSnapshots)

    votingStakePerPool =
        querySPOStakeDistr nextEpochState Set.empty

    registeredDRepStates =
        queryDRepState targetEpochState Set.empty

    dRepStakeDistribution =
        queryDRepStakeDistr nextEpochState Set.empty

    accountSummaries =
        Map.mapWithKey (mkAccountSummary instantStake dRepDelegatees) accountsMap

    accountsMap =
        epochStateForAccounts ^. esLStateL . lsCertStateL . certDStateL . accountsL . accountsMapL

    instantStake =
        targetEpochState ^. instantStakeL . instantStakeCredentialsL

    dRepDelegatees =
        Map.fromList
            [ (credential, drep)
            | (drep, delegators) <- Map.toAscList (queryDRepDelegations targetEpochState Set.empty)
            , credential <- Set.toAscList delegators
            ]

    epochStateForAccounts =
        case nesRu targetEpochState of
            SNothing ->
                targetEpochState ^. nesEsL
            SJust pulsingRewardUpdate ->
                applyRUpd (completeRewardUpdate pulsingRewardUpdate) (targetEpochState ^. nesEsL)

    completeRewardUpdate pulsingRewardUpdate =
        fst $
            runIdentity $
                runReaderT
                    (completeRupd pulsingRewardUpdate)
                    (error "completeRupd unexpectedly forced Globals while building a stake distribution")
