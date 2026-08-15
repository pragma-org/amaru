{-# LANGUAGE DataKinds #-}
{-# LANGUAGE DeriveGeneric #-}
{-# LANGUAGE DuplicateRecordFields #-}

module Data.Fixture.InitialState
    ( Account (..)
    , CertificatePointer (..)
    , GovernanceActivity (..)
    , InitialState (..)
    , PoolDelegation (..)
    , Pots (..)
    , ProposalEntry (..)
    , RegisteredDRep (..)
    , VoteDelegation (..)
    , buildNewEpochState
    ) where

import Relude

import Cardano.Ledger.Api.Governance
    ( cgsProposalsL
    , curPParamsGovStateL
    , emptyGovState
    , prevPParamsGovStateL
    )
import Cardano.Ledger.Api.PParams
    ( PParams
    , emptyPParamsUpdate
    , ppPoolDepositL
    )
import Cardano.Ledger.BaseTypes
    ( ProtVer (..)
    , addEpochInterval
    )
import Cardano.Ledger.Binary.Version
    ( mkVersion
    )
import Cardano.Ledger.Coin
    ( Coin (..)
    )
import Cardano.Ledger.Compactible
    ( CompactForm
    , Compactible (fromCompact)
    )
import Cardano.Ledger.Conway
    ( ConwayEra
    )
import Cardano.Ledger.Conway.Governance
    ( Committee (..)
    , GovAction (..)
    , GovActionId (GovActionId)
    , GovActionIx (GovActionIx)
    , GovActionState (..)
    , GovPurposeId (GovPurposeId)
    , GovRelation (..)
    , ProposalProcedure (..)
    , Proposals
    , cgsCommitteeL
    , cgsConstitutionL
    , constitutionGuardrailsScriptHashL
    , fromPrevGovActionIds
    , gasDeposit
    , pRootsL
    , proposalsAddAction
    )
import Cardano.Ledger.Conway.PParams
    ( ppGovActionDepositL
    , ppGovActionLifetimeL
    )
import Cardano.Ledger.Conway.State
    ( ConwayAccountState (..)
    , ConwayAccounts (..)
    , ConwayCertState (..)
    , VState (..)
    )
import Cardano.Ledger.Core
    ( TxOut
    )
import Cardano.Ledger.Credential
    ( Credential
    )
import Cardano.Ledger.DRep
    ( DRep (DRepCredential)
    , DRepState (..)
    )
import Cardano.Ledger.Hashes
    ( GenDelegs (GenDelegs)
    , KeyHash
    , ScriptHash
    )
import Cardano.Ledger.Keys
    ( KeyRole (ColdCommitteeRole, DRepRole, StakePool, Staking)
    )
import qualified Cardano.Ledger.Slot as LedgerSlot
import Cardano.Ledger.Shelley.LedgerState
    ( EpochState (..)
    , LedgerState (..)
    , NewEpochState (..)
    , UTxO (..)
    , UTxOState (..)
    )
import Cardano.Ledger.State
    ( ChainAccountState (..)
    , CommitteeAuthorization (CommitteeHotCredential)
    , CommitteeState (CommitteeState)
    , DState (..)
    , PState (..)
    , StakePoolState
    , spsDepositL
    )
import Cardano.Ledger.TxIn
    ( TxId
    , TxIn
    )
import Command.ValidatePhaseOne.Error
    ( Error (..)
    )
import Data.Aeson
    ( FromJSON (parseJSON)
    , Object
    , Value (Array)
    , withObject
    , withText
    , (.:)
    , (.:?)
    , (.!=)
    )
import Data.Aeson.Key
    ( Key
    )
import Data.Aeson.Types
    ( Parser
    )
import Data.Default.Class
    ( Default (def)
    )
import Data.Fixture.Common
    ( compactCoin
    , compactCoinOrError
    , parseCborHex
    , parsePoolId
    , parseScriptHash
    , showText
    )
import Data.Fixture.EraHistory
    ( EraHistory
    , pointEpochNo
    )
import Data.Fixture.Point
    ( Point
    )
import Data.Maybe.Strict
    ( StrictMaybe (SJust, SNothing)
    , maybeToStrictMaybe
    )
import Lens.Micro
    ( (.~)
    , (^.)
    )

import qualified Data.Map.Strict as Map
import qualified Data.Set as Set
import qualified Data.Text as Text

data InitialState = InitialState
    { utxo :: ![UtxoEntry]
    , pools :: ![KeyHash StakePool]
    , accounts :: ![Account]
    , dreps :: ![RegisteredDRep]
    , committee :: ![CommitteeMember]
    , proposals :: ![ProposalEntry]
    , proposalsRoots :: !(GovRelation StrictMaybe)
    , pots :: !Pots
    , governanceActivity :: !GovernanceActivity
    , guardrailScript :: !(Maybe ScriptHash)
    }
    deriving (Generic)

instance FromJSON InitialState where
    parseJSON =
        withObject "InitialState" $ \objectValue ->
            InitialState
                <$> objectValue .:? "utxo" .!= []
                <*> (objectValue .:? "pools" .!= [] >>= traverse parsePoolId)
                <*> objectValue .:? "accounts" .!= []
                <*> objectValue .:? "dreps" .!= []
                <*> objectValue .:? "committee" .!= []
                <*> objectValue .:? "proposals" .!= []
                <*> (objectValue .:? "proposalsRoots" >>= maybe (pure def) parseProposalsRoots)
                <*> objectValue .:? "pots" .!= def
                <*> objectValue .:? "governanceActivity" .!= def
                <*> (objectValue .:? "guardrailScript" >>= traverse parseScriptHash)

data UtxoEntry = UtxoEntry
    { input :: !TxIn
    , output :: !(TxOut ConwayEra)
    }

instance FromJSON UtxoEntry where
    parseJSON =
        withObject "UtxoEntry" $ \objectValue ->
            UtxoEntry
                <$> (objectValue .: "input" >>= parseCborHex "TxIn")
                <*> (objectValue .: "output" >>= parseCborHex "TxOut")

data Account = Account
    { credential :: !(Credential Staking)
    , deposit :: !Integer
    , rewards :: !Integer
    , pool :: !(Maybe PoolDelegation)
    , drep :: !(Maybe VoteDelegation)
    }
    deriving (Generic)

instance FromJSON Account where
    parseJSON =
        withObject "Account" $ \objectValue ->
            Account
                <$> (objectValue .: "credential" >>= parseCborHex "StakeCredential")
                <*> objectValue .: "deposit"
                <*> objectValue .:? "rewards" .!= 0
                <*> objectValue .:? "pool"
                <*> objectValue .:? "drep"

data PoolDelegation = PoolDelegation
    { poolId :: !(KeyHash StakePool)
    , delegatedAt :: !CertificatePointer
    }

instance FromJSON PoolDelegation where
    parseJSON =
        withObject "PoolDelegation" $ \objectValue ->
            PoolDelegation
                <$> (objectValue .: "id" >>= parsePoolId)
                <*> objectValue .: "delegatedAt"

data VoteDelegation = VoteDelegation
    { delegatedDRep :: !DRep
    , delegatedAt :: !CertificatePointer
    }

instance FromJSON VoteDelegation where
    parseJSON =
        withObject "VoteDelegation" $ \objectValue ->
            VoteDelegation
                <$> (objectValue .: "id" >>= parseCborHex "DRep")
                <*> objectValue .: "delegatedAt"

data RegisteredDRep = RegisteredDRep
    { credential :: !(Credential Staking)
    , deposit :: !Integer
    , registeredAt :: !CertificatePointer
    , validUntil :: !Word64
    }

instance FromJSON RegisteredDRep where
    parseJSON =
        withObject "RegisteredDRep" $ \objectValue ->
            RegisteredDRep
                <$> (objectValue .: "credential" >>= parseCborHex "StakeCredential")
                <*> objectValue .: "deposit"
                <*> objectValue .: "registeredAt"
                <*> objectValue .: "validUntil"

-- | A seeded constitutional committee row keyed by cold credential. `status = null` means the
-- member has not authorized any hot credential yet. `validUntil = null` means the member currently
-- holds no elected seat, even if they still have an authorization recorded.
data CommitteeMember = CommitteeMember
    { coldCredential :: !(Credential ColdCommitteeRole)
    , status :: !(Maybe CommitteeAuthorization)
    , validUntil :: !(Maybe Word64)
    }

instance FromJSON CommitteeMember where
    parseJSON =
        withObject "CommitteeMember" $ \objectValue ->
            CommitteeMember
                <$> (objectValue .: "coldCredential" >>= parseCborHex "ColdCommitteeCredential")
                <*> (objectValue .:? "status" >>= traverse parseCommitteeAuthorization)
                <*> objectValue .:? "validUntil"

data CertificatePointer = CertificatePointer
    { transaction :: !Point
    , certificateIndex :: !Word64
    }
    deriving (Generic)

instance FromJSON CertificatePointer

data GovernanceActivity = GovernanceActivity
    { consecutiveDormantEpochs :: !Word64
    }
    deriving (Generic)

instance FromJSON GovernanceActivity

instance Default GovernanceActivity where
    def = GovernanceActivity 0

data ProposalEntry = ProposalEntry
    { proposalId :: !GovActionId
    , proposalAction :: !ProposalAction
    , proposalValidUntil :: !(Maybe Word64)
    }

-- | What a fixture states about a seeded proposal's governance action: the action itself, or
-- only the lineage it belongs to.
data ProposalAction
    = ProposalFull !(GovAction ConwayEra)
    | ProposalSlim !ProposalSlim

-- | The purposes proposals chain along. @Orphan@ covers the actions that chain to nothing.
data ProposalSlim
    = ProtocolParametersSlim
    | HardForkSlim !ProtVer
    | ConstitutionalCommitteeSlim
    | ConstitutionSlim
    | OrphanSlim

instance FromJSON ProposalSlim where
    parseJSON =
        withText "ProposalSlim" $ \text ->
            case text of
                "ProtocolParameters" ->
                    pure ProtocolParametersSlim
                "ConstitutionalCommittee" ->
                    pure ConstitutionalCommitteeSlim
                "Constitution" ->
                    pure ConstitutionSlim
                "Orphan" ->
                    pure OrphanSlim
                other ->
                    case parseHardForkSlim other of
                        Right proposalSlim ->
                            pure proposalSlim
                        Left err ->
                            fail (toString err)
     where
       parseHardForkSlim :: Text -> Either Text ProposalSlim
       parseHardForkSlim text = do
           versionText <-
               maybe
                   (Left ("failed to parse proposal from string: " <> text))
                   Right
                   (Text.stripSuffix ")" =<< Text.stripPrefix "HardFork(" text)
           protocolVersion <- parseProtocolVersion versionText
           pure (HardForkSlim protocolVersion)


instance FromJSON ProposalEntry where
    parseJSON value =
        case value of
            Array _ ->
                parseSlimPair value
            _ ->
                parseFullEntry value
      where
        parseSlimPair =
            parseJSON >=> \(idValue, lineage) -> do
                entryId <- parseProposalId idValue
                pure ProposalEntry{proposalId = entryId, proposalAction = ProposalSlim lineage, proposalValidUntil = Nothing}
        parseFullEntry =
            withObject "ProposalEntry" $ \objectValue ->
                ProposalEntry
                    <$> (objectValue .: "id" >>= parseProposalId)
                    <*> (ProposalFull <$> (objectValue .: "govAction" >>= parseCborHex "GovAction"))
                    <*> objectValue .:? "validUntil"

data Pots = Pots
    { treasury :: !Integer
    , reserves :: !Integer
    }

instance FromJSON Pots where
    parseJSON =
        withObject "Pots" $ \objectValue ->
            Pots
                <$> objectValue .:? "treasury" .!= 0
                <*> objectValue .:? "reserves" .!= 0

instance Default Pots where
    def = Pots 0 0

parseProposalId :: Value -> Parser GovActionId
parseProposalId =
    withObject "ProposalId" $ \objectValue -> do
        transactionId <- objectValue .: "transactionId" :: Parser TxId
        proposalIndex <- objectValue .: "proposalIndex"
        pure (GovActionId transactionId (GovActionIx proposalIndex))

-- | The latest enacted governance action id per purpose. An absent purpose has no root,
-- so a proposal claiming it as parent must supply an empty parent to be accepted.
parseProposalsRoots :: Value -> Parser (GovRelation StrictMaybe)
parseProposalsRoots =
    withObject "ProposalsRoots" $ \objectValue -> do
        grPParamUpdate <- parseRoot objectValue "protocolParameters"
        grHardFork <- parseRoot objectValue "hardFork"
        grCommittee <- parseRoot objectValue "constitutionalCommittee"
        grConstitution <- parseRoot objectValue "constitution"
        pure GovRelation{grPParamUpdate, grHardFork, grCommittee, grConstitution}
  where
    parseRoot :: Object -> Key -> Parser (StrictMaybe (GovPurposeId purpose))
    parseRoot objectValue key =
        objectValue .:? key
            >>= maybe (pure SNothing) (fmap (SJust . GovPurposeId) . parseProposalId)

buildNewEpochState
    :: PParams ConwayEra
    -> EraHistory
    -> InitialState
    -> Point
    -> Either Error (NewEpochState ConwayEra)
buildNewEpochState pparams eraHistory initialState point = do
    let InitialState{utxo, pools, accounts, dreps, committee, proposals, proposalsRoots, pots, governanceActivity, guardrailScript} = initialState
    accountEntries <- traverse toLedgerAccountEntry accounts
    dRepEntries <- traverse toLedgerDRepEntry dreps
    currentEpoch <- pointEpochNo eraHistory point
    let proposalStates = map (toGovActionState pparams currentEpoch) proposals
    seededProposals <- buildProposals proposalsRoots proposalStates

    let accountsMap = Map.fromList accountEntries
    let dRepDelegators =
            Map.fromListWith (<>)
                [ (credential, Set.singleton stakingCredential)
                | (stakingCredential, AccountState{accountDRep = Just delegatedDRep}) <- accountEntries
                , DRepCredential credential <- [delegatedDRep]
                ]
    let dRepStates =
            Map.fromList
                [ ( registeredDRepCredential
                  , DRepState
                        { drepExpiry = phaseOneEpochNo registeredDRepValidUntil
                        , drepAnchor = SNothing
                        , drepDeposit = registeredDRepDeposit
                        , drepDelegs = Map.findWithDefault mempty registeredDRepCredential dRepDelegators
                        }
                  )
                | RegisteredDRepState
                    { registeredDRepCredential
                    , registeredDRepDeposit
                    , registeredDRepValidUntil
                    }
                    <- dRepEntries
                ]
    let electedCommitteeMembers =
            Map.fromList
                [ (coldCredential, phaseOneEpochNo memberValidUntil)
                | CommitteeMember{coldCredential, validUntil = Just memberValidUntil} <- committee
                ]
    let committeeAuthorizations =
            Map.fromList
                [ (coldCredential, authorization)
                | CommitteeMember{coldCredential, status = Just authorization} <- committee
                ]
    let poolStates =
            Map.fromList
                [ (poolId, defaultStakePoolState (Coin (poolDepositAmount pparams)))
                | poolId <- pools
                ]
    let deposited =
            Coin
                ( sum
                    [ accountDepositValue
                    | (_, AccountState{accountDeposit}) <- accountEntries
                    , let Coin accountDepositValue = fromCompact accountDeposit
                    ]
                    + sum
                        [ dRepDepositValue
                        | RegisteredDRepState{registeredDRepDeposit} <- dRepEntries
                        , let Coin dRepDepositValue = fromCompact registeredDRepDeposit
                        ]
                    + (fromIntegral (length pools) * poolDepositAmount pparams)
                    + sum
                        [ proposalDepositValue
                        | proposalState <- proposalStates
                        , let Coin proposalDepositValue = gasDeposit proposalState
                        ]
                )
    let govState =
            emptyGovState
                & curPParamsGovStateL .~ pparams
                & prevPParamsGovStateL .~ pparams
                & cgsCommitteeL
                    .~ ( if Map.null electedCommitteeMembers
                            then SNothing
                            else SJust (def{committeeMembers = electedCommitteeMembers})
                       )
                & cgsProposalsL .~ seededProposals
                & cgsConstitutionL . constitutionGuardrailsScriptHashL .~ maybeToStrictMaybe guardrailScript
    let chainAccountState =
            ChainAccountState {casTreasury = Coin (treasury pots), casReserves = Coin (reserves pots)}
    let utxoState =
            UTxOState
                { utxosUtxo = UTxO (Map.fromList [(input entry, output entry) | entry <- utxo])
                , utxosDeposited = deposited
                , utxosFees = Coin 0
                , utxosGovState = govState
                , utxosInstantStake = def
                , utxosDonation = Coin 0
                }
    let certState =
            ConwayCertState
                { conwayCertVState =
                    VState
                        { vsDReps = dRepStates
                        , vsCommitteeState = CommitteeState committeeAuthorizations
                        , vsNumDormantEpochs = phaseOneEpochNo (consecutiveDormantEpochs governanceActivity)
                        }
                , conwayCertPState =
                    PState def poolStates def def
                , conwayCertDState =
                    DState
                        { dsAccounts =
                            ConwayAccounts
                                ( Map.map
                                    ( \AccountState{accountBalance, accountDeposit, accountPool, accountDRep} ->
                                        ConwayAccountState
                                            { casBalance = accountBalance
                                            , casDeposit = accountDeposit
                                            , casStakePoolDelegation = accountPool
                                            , casDRepDelegation = accountDRep
                                            }
                                    )
                                    accountsMap
                                )
                        , dsFutureGenDelegs = def
                        , dsGenDelegs = GenDelegs mempty
                        , dsIRewards = def
                        }
                }
    let ledgerState = LedgerState utxoState certState
    let epochState =
            EpochState
                { esChainAccountState = chainAccountState
                , esLState = ledgerState
                , esSnapshots = def
                , esNonMyopic = def
                }

    pure
        NewEpochState
            { nesEL = currentEpoch
            , nesBprev = def
            , nesBcur = def
            , nesEs = epochState
            , nesRu = SNothing
            , nesPd = def
            , stashedAVVMAddresses = ()
            }

data AccountState = AccountState
    { accountBalance :: !(CompactForm Coin)
    , accountDeposit :: !(CompactForm Coin)
    , accountPool :: !(Maybe (KeyHash StakePool))
    , accountDRep :: !(Maybe DRep)
    }

data RegisteredDRepState = RegisteredDRepState
    { registeredDRepCredential :: !(Credential DRepRole)
    , registeredDRepDeposit :: !(CompactForm Coin)
    , registeredDRepValidUntil :: !Word64
    }

toLedgerAccountEntry :: Account -> Either Error (Credential Staking, AccountState)
toLedgerAccountEntry Account{credential, deposit, rewards, pool, drep} = do
    compactDeposit <- first UnsupportedFixture (compactCoin "account deposit" (Coin deposit))
    compactBalance <- first UnsupportedFixture (compactCoin "account rewards" (Coin rewards))
    pure
        ( credential
        , AccountState
            { accountBalance = compactBalance
            , accountDeposit = compactDeposit
            , accountPool = poolId <$> pool
            , accountDRep = delegatedDRep <$> drep
            }
        )

toLedgerDRepEntry :: RegisteredDRep -> Either Error RegisteredDRepState
toLedgerDRepEntry RegisteredDRep{credential, deposit, validUntil} = do
    compactDeposit <- first UnsupportedFixture (compactCoin "delegate representative deposit" (Coin deposit))
    pure
        RegisteredDRepState
            { registeredDRepCredential = toDRepCredential credential
            , registeredDRepDeposit = compactDeposit
            , registeredDRepValidUntil = validUntil
            }

-- | A fixture describes a seeded proposal by its id and its governance action, which is
-- all the GOV rule consults when it resolves a new proposal's parent. The deposit, return
-- address and anchor are filler. A fixture that states no expiry gets one derived from
-- @govActionLifetime@, so that the proposal counts as still in flight.
toGovActionState :: PParams ConwayEra -> LedgerSlot.EpochNo -> ProposalEntry -> GovActionState ConwayEra
toGovActionState pparams currentEpoch ProposalEntry{proposalId, proposalAction, proposalValidUntil} =
    GovActionState
        { gasId = proposalId
        , gasCommitteeVotes = mempty
        , gasDRepVotes = mempty
        , gasStakePoolVotes = mempty
        , gasProposalProcedure =
            ProposalProcedure
                { pProcDeposit = pparams ^. ppGovActionDepositL
                , pProcReturnAddr = def
                , pProcGovAction = toGovAction proposalAction
                , pProcAnchor = def
                }
        , gasProposedIn = currentEpoch
        , gasExpiresAfter =
            maybe
                (addEpochInterval currentEpoch (pparams ^. ppGovActionLifetimeL))
                phaseOneEpochNo
                proposalValidUntil
        }

-- | A fixture naming only a slim constrains nothing beyond the purpose its proposal chains
-- along, so a minimal action of that purpose stands in. The stand-in hard fork sits at the
-- current protocol version, the weakest parent a chaining proposal can still follow.
toGovAction :: ProposalAction -> GovAction ConwayEra
toGovAction = \case
    ProposalFull govAction ->
        govAction
    ProposalSlim ProtocolParametersSlim ->
        ParameterChange SNothing emptyPParamsUpdate SNothing
    ProposalSlim (HardForkSlim version) ->
        HardForkInitiation SNothing version
    ProposalSlim ConstitutionalCommitteeSlim ->
        NoConfidence SNothing
    ProposalSlim ConstitutionSlim ->
        NewConstitution SNothing def
    ProposalSlim OrphanSlim ->
        InfoAction

buildProposals
    :: GovRelation StrictMaybe
    -> [GovActionState ConwayEra]
    -> Either Error (Proposals ConwayEra)
buildProposals roots =
    foldlM addAction (def & pRootsL .~ fromPrevGovActionIds roots)
  where
    addAction acc proposalState =
        maybe (Left (unplaceable proposalState)) Right (proposalsAddAction proposalState acc)
    unplaceable proposalState =
        UnsupportedFixture
            ( "initial proposal "
                <> showText (gasId proposalState)
                <> " does not follow an enacted root or an earlier initial proposal"
            )

defaultStakePoolState :: Coin -> StakePoolState
defaultStakePoolState depositCoin =
    def
        & spsDepositL .~ compactCoinOrError "stakePoolDeposit" depositCoin

poolDepositAmount :: PParams ConwayEra -> Integer
poolDepositAmount pparams =
    case pparams ^. ppPoolDepositL of
        Coin amount ->
            amount

phaseOneEpochNo :: Word64 -> LedgerSlot.EpochNo
phaseOneEpochNo =
    LedgerSlot.EpochNo

toDRepCredential :: Credential Staking -> Credential DRepRole
toDRepCredential =
    coerce

parseCommitteeAuthorization :: Value -> Parser CommitteeAuthorization
parseCommitteeAuthorization =
    withText "CommitteeAuthorization" $ \hexText ->
        parseCborHex "CommitteeAuthorization" hexText
            <|> (CommitteeHotCredential <$> parseCborHex "HotCommitteeCredential" hexText)

parseProtocolVersion :: Text -> Either Text ProtVer
parseProtocolVersion versionText =
    case Text.splitOn "." versionText of
        [majorText, minorText] -> do
            majorWord <- parseWord64 "hard fork version major" majorText
            minorWord <- parseWord64 "hard fork version minor" minorText
            majorVersion <-
                maybe
                    (Left ("hard fork version major is out of bounds: " <> showText majorWord))
                    Right
                    (mkVersion majorWord)
            pure (ProtVer majorVersion (fromIntegral minorWord))
        _ ->
            Left ("failed to parse hard fork version: " <> versionText <> "; expected <major>.<minor>")

parseWord64 :: Text -> Text -> Either Text Word64
parseWord64 contextLabel rawText =
    maybe
        (Left ("failed to parse " <> contextLabel <> ": " <> rawText))
        Right
        (readMaybe (toString rawText))
