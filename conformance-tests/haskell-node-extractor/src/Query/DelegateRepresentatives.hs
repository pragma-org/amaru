{-# LANGUAGE DataKinds #-}
{-# LANGUAGE DuplicateRecordFields #-}
{-# LANGUAGE NamedFieldPuns #-}

module Query.DelegateRepresentatives
    ( delegateRepresentativesOutputPath
    , queryDelegateRepresentatives
    ) where

import Relude

import Cardano.Ledger.Api.State.Query
    ( queryDRepDelegations
    , queryDRepStakeDistr
    , queryDRepState
    )
import Cardano.Ledger.Coin
    ( Coin
    )
import Cardano.Ledger.Compactible
    ( fromCompact
    )
import Cardano.Ledger.Credential
    ( Credential
    )
import Cardano.Ledger.Keys
    ( KeyRole (..)
    )
import Cardano.Ledger.DRep
    ( DRep
        ( DRepAlwaysAbstain
        , DRepAlwaysNoConfidence
        )
    , DRepState
        ( drepDeposit
        , drepExpiry
        )
    , credToDRep
    )
import Cardano.Ledger.Shelley.LedgerState
    ( NewEpochState
    )
import Data.DelegateRepresentative
    ( DelegateRepresentative (..)
    , PredefinedDRep (..)
    , RegisteredDRep (..)
    )
import Data.Mandate
    ( Mandate (Mandate)
    )
import Ouroboros.Consensus.Cardano.Block
    ( ConwayEra
    )

import qualified Data.Map.Merge.Strict as Merge
import qualified Data.Map.Strict as Map
import qualified Data.Set as Set

delegateRepresentativesOutputPath :: Word64 -> FilePath
delegateRepresentativesOutputPath epochNumber =
    "data/dreps/" <> toString (show epochNumber :: Text) <> ".json"

queryDelegateRepresentatives :: NewEpochState ConwayEra -> [DelegateRepresentative]
queryDelegateRepresentatives newEpochState =
    Map.elems $
        mergeDelegations
            ( mergeStateAndStake
                registeredDRepStates
                dRepStakeDistribution
            )
            dRepDelegations
  where
    registeredDRepStates =
        Map.fromList
            [ (credToDRep credential, (credential, st))
            | (credential, st) <- Map.toList (queryDRepState newEpochState Set.empty)
            ]
    dRepStakeDistribution =
        queryDRepStakeDistr newEpochState Set.empty
    dRepDelegations =
        queryDRepDelegations newEpochState Set.empty

mergeStateAndStake
    :: Map.Map DRep (Credential DRepRole, DRepState)
    -> Map.Map DRep Coin
    -> Map.Map DRep DelegateRepresentative
mergeStateAndStake dRepStates dRepStakes =
    Merge.merge
        (Merge.mapMissing $ \_ (credential, dRepState) -> registeredDRep credential dRepState mempty)
        (Merge.mapMissing $ \drep -> predefinedDRep drep)
        (Merge.zipWithMatched $ \_ (credential, dRepState) -> registeredDRep credential dRepState)
        dRepStates
        dRepStakes

mergeDelegations
    :: Map.Map DRep DelegateRepresentative
    -> Map.Map DRep (Set.Set (Credential Staking))
    -> Map.Map DRep DelegateRepresentative
mergeDelegations dReps dRepDelegations =
    Merge.merge
        (Merge.mapMissing $ \_ -> identity)
        (Merge.mapMaybeMissing $ \drep delegators ->
            -- DReps with delegators but no stake/state entry are either
            -- predefined DReps with zero stake or retired registered DReps.
            -- Retired registered DReps are dropped from the output.
            case drep of
                DRepAlwaysAbstain ->
                    Just $ setDelegators delegators (AbstainDelegateRepresentative PredefinedDRep{stake = mempty, delegators = Set.empty})
                DRepAlwaysNoConfidence ->
                    Just $ setDelegators delegators (NoConfidenceDelegateRepresentative PredefinedDRep{stake = mempty, delegators = Set.empty})
                _ ->
                    Nothing
        )
        (Merge.zipWithMatched $ \_ delegateRepresentative delegators -> setDelegators delegators delegateRepresentative)
        dReps
        dRepDelegations

registeredDRep
    :: Credential DRepRole
    -> DRepState
    -> Coin
    -> DelegateRepresentative
registeredDRep credential dRepState stake =
    RegisteredDelegateRepresentative
        RegisteredDRep
            { credential
            , mandate = Mandate (drepExpiry dRepState)
            , deposit = fromCompact (drepDeposit dRepState)
            , stake
            , delegators = Set.empty
            }

predefinedDRep :: DRep -> Coin -> DelegateRepresentative
predefinedDRep drep stake = case drep of
    DRepAlwaysAbstain ->
        AbstainDelegateRepresentative PredefinedDRep{stake, delegators = Set.empty}
    DRepAlwaysNoConfidence ->
        NoConfidenceDelegateRepresentative PredefinedDRep{stake, delegators = Set.empty}
    _ ->
        AbstainDelegateRepresentative PredefinedDRep{stake, delegators = Set.empty}

setDelegators
    :: Set.Set (Credential Staking)
    -> DelegateRepresentative
    -> DelegateRepresentative
setDelegators delegators = \case
    RegisteredDelegateRepresentative RegisteredDRep{credential, deposit, mandate, stake} ->
        RegisteredDelegateRepresentative RegisteredDRep{credential, mandate, deposit, stake, delegators}
    AbstainDelegateRepresentative PredefinedDRep{stake} ->
        AbstainDelegateRepresentative PredefinedDRep{stake, delegators}
    NoConfidenceDelegateRepresentative PredefinedDRep{stake} ->
        NoConfidenceDelegateRepresentative PredefinedDRep{stake, delegators}
