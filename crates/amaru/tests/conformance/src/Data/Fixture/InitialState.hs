{-# LANGUAGE DataKinds #-}
{-# LANGUAGE DeriveGeneric #-}
{-# LANGUAGE DuplicateRecordFields #-}

module Data.Fixture.InitialState
    ( Account (..)
    , CertificatePointer (..)
    , GovernanceActivity (..)
    , InitialState (..)
    , PoolDelegation (..)
    , RegisteredDRep (..)
    , VoteDelegation (..)
    , buildNewEpochState
    ) where

import Relude

import Cardano.Ledger.Api.Governance
    ( curPParamsGovStateL
    , emptyGovState
    , prevPParamsGovStateL
    )
import Cardano.Ledger.Api.PParams
    ( PParams
    , ppPoolDepositL
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
    )
import Cardano.Ledger.Keys
    ( KeyRole (DRepRole, StakePool, Staking)
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
    , DState (..)
    , PState (..)
    , StakePoolState
    , spsDepositL
    )
import Cardano.Ledger.TxIn
    ( TxIn
    )
import Command.ValidatePhaseOne.Error
    ( Error (..)
    )
import Data.Aeson
    ( FromJSON (parseJSON)
    , withObject
    , (.:)
    , (.:?)
    , (.!=)
    )
import Data.Default.Class
    ( Default (def)
    )
import Data.Fixture.Common
    ( compactCoin
    , compactCoinOrError
    , parseCborHex
    , parsePoolId
    )
import Data.Fixture.EraHistory
    ( EraHistory
    , pointEpochNo
    )
import Data.Fixture.Point
    ( Point
    )
import Data.Maybe.Strict
    ( StrictMaybe (SNothing)
    )
import Lens.Micro
    ( (.~)
    , (^.)
    )

import qualified Data.Map.Strict as Map
import qualified Data.Set as Set

data InitialState = InitialState
    { utxo :: ![UtxoEntry]
    , pools :: ![KeyHash StakePool]
    , accounts :: ![Account]
    , dreps :: ![RegisteredDRep]
    , governanceActivity :: !GovernanceActivity
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
                <*> objectValue .: "governanceActivity"

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

buildNewEpochState
    :: PParams ConwayEra
    -> EraHistory
    -> InitialState
    -> Point
    -> Either Error (NewEpochState ConwayEra)
buildNewEpochState pparams eraHistory InitialState{utxo, pools, accounts, dreps, governanceActivity} point = do
    accountEntries <- traverse toLedgerAccountEntry accounts
    dRepEntries <- traverse toLedgerDRepEntry dreps
    currentEpoch <- pointEpochNo eraHistory point

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
                )
    let govState =
            emptyGovState
                & curPParamsGovStateL .~ pparams
                & prevPParamsGovStateL .~ pparams
    let chainAccountState = ChainAccountState {casTreasury = Coin 0, casReserves = Coin 0}
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
                        , vsCommitteeState = def
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
