{-# LANGUAGE ExplicitNamespaces #-}
{-# LANGUAGE PatternSynonyms #-}

module Snapshot
    ( LoadedSnapshot (..)
    , loadSnapshot
    ) where

import Relude

import Cardano.Chain.Slotting
    ( EpochSlots (..)
    )
import Cardano.Ledger.Shelley.LedgerState
    ( NewEpochState
    , nesEL
    )
import Cardano.Slotting.Slot
    ( EpochNo (unEpochNo)
    , SlotNo (unSlotNo)
    , WithOrigin (At, Origin)
    )
import Codec.Serialise.Class
    ( decode
    )
import Command.ExtractSnapshot
    ( ExtractSnapshotOptions (..)
    )
import Error
    ( AppError (..)
    )
import Ouroboros.Consensus.Byron.Ledger
    ( CodecConfig (ByronCodecConfig)
    )
import Ouroboros.Consensus.Byron.Ledger.Ledger
    ()
import Ouroboros.Consensus.Byron.Node.Serialisation
    ()
import Ouroboros.Consensus.Cardano.Block
    ( CardanoCodecConfig
    , CardanoChainDepState
    , CardanoLedgerState
    , CodecConfig (CardanoCodecConfig)
    , ConwayEra
    , LedgerState
        ( LedgerStateAllegra
        , LedgerStateAlonzo
        , LedgerStateBabbage
        , LedgerStateByron
        , LedgerStateConway
        , LedgerStateMary
        , LedgerStateShelley
        )
    , StandardCrypto
    , pattern ChainDepStateAllegra
    , pattern ChainDepStateAlonzo
    , pattern ChainDepStateBabbage
    , pattern ChainDepStateByron
    , pattern ChainDepStateConway
    , pattern ChainDepStateMary
    , pattern ChainDepStateShelley
    )
import Ouroboros.Consensus.Cardano.Node
    ()
import Ouroboros.Consensus.HardFork.Combinator.Serialisation.SerialiseDisk
    ()
import Ouroboros.Consensus.HeaderValidation
    ( AnnTip (annTipSlotNo)
    , headerStateChainDep
    , headerStateTip
    )
import Ouroboros.Consensus.Ledger.Extended
    ( ExtLedgerState (headerState, ledgerState)
    , decodeDiskExtLedgerState
    )
import Ouroboros.Consensus.Protocol.Praos
    ( PraosState
    )
import Ouroboros.Consensus.Shelley.Ledger
    ( CodecConfig (ShelleyCodecConfig)
    , shelleyLedgerState
    )
import Ouroboros.Consensus.Shelley.Ledger.SupportsProtocol
    ()
import Ouroboros.Consensus.Shelley.Node.Serialisation
    ()
import Ouroboros.Consensus.Storage.LedgerDB.Snapshots
    ( readExtLedgerState
    )
import System.Directory
    ( makeAbsolute
    )
import System.FS.API
    ( HasFS
    , SomeHasFS (..)
    )
import System.FS.API.Types
    ( FsPath
    , MountPoint (..)
    , mkFsPath
    )
import System.FS.IO
    ( HandleIO
    , ioHasFS
    )
import System.FilePath
    ( makeRelative
    , normalise
    , splitDirectories
    )

data LoadedSnapshot = LoadedSnapshot
    { loadedSnapshotEpochNumber :: !Word64
    , loadedSnapshotPraosState :: !PraosState
    , loadedSnapshotState :: !(NewEpochState ConwayEra)
    , loadedSnapshotTipSlot :: !Word64
    }

loadSnapshot :: ExtractSnapshotOptions -> ExceptT AppError IO LoadedSnapshot
loadSnapshot ExtractSnapshotOptions{snapshotPath} = do
    absoluteSnapshotPath <- liftIO (makeAbsolute snapshotPath)
    let filesystem = SomeHasFS (ioHasFS rootMountPoint :: HasFS IO HandleIO)
    let snapshotFsPath = toFsPath absoluteSnapshotPath

    snapshotResult <-
        liftIO
            ( runExceptT
                (readExtLedgerState filesystem (decodeDiskExtLedgerState cardanoCodecConfig) decode snapshotFsPath)
            )

    (extLedgerState, _checksum) <-
        hoistEither (first (SnapshotReadError absoluteSnapshotPath) snapshotResult)

    extractedState <-
        hoistEither
            (extractConwayNewEpochState absoluteSnapshotPath (ledgerState extLedgerState))

    praosState <-
        hoistEither
            (extractConwayPraosState absoluteSnapshotPath (headerStateChainDep (headerState extLedgerState)))

    tipSlot <-
        hoistEither
            (maybe
                (Left (SnapshotHasNoTip absoluteSnapshotPath))
                Right
                (extractTipSlot $ headerStateTip (headerState extLedgerState))
            )

    pure
        LoadedSnapshot
            { loadedSnapshotEpochNumber = extractEpochNumber extractedState
            , loadedSnapshotPraosState = praosState
            , loadedSnapshotState = extractedState
            , loadedSnapshotTipSlot = tipSlot
            }

extractEpochNumber :: NewEpochState ConwayEra -> Word64
extractEpochNumber newEpochState =
    unEpochNo (nesEL newEpochState)

extractConwayNewEpochState
    :: FilePath
    -> CardanoLedgerState StandardCrypto mk
    -> Either AppError (NewEpochState ConwayEra)
extractConwayNewEpochState snapshotFilePath = \case
    LedgerStateByron _ ->
        Left (UnsupportedSnapshotEra snapshotFilePath "Byron")
    LedgerStateShelley _ ->
        Left (UnsupportedSnapshotEra snapshotFilePath "Shelley")
    LedgerStateAllegra _ ->
        Left (UnsupportedSnapshotEra snapshotFilePath "Allegra")
    LedgerStateMary _ ->
        Left (UnsupportedSnapshotEra snapshotFilePath "Mary")
    LedgerStateAlonzo _ ->
        Left (UnsupportedSnapshotEra snapshotFilePath "Alonzo")
    LedgerStateBabbage _ ->
        Left (UnsupportedSnapshotEra snapshotFilePath "Babbage")
    LedgerStateConway ledgerSt ->
        Right (shelleyLedgerState ledgerSt)

extractConwayPraosState
    :: FilePath
    -> CardanoChainDepState StandardCrypto
    -> Either AppError PraosState
extractConwayPraosState snapshotFilePath = \case
    ChainDepStateByron _ ->
        Left (UnsupportedSnapshotEra snapshotFilePath "Byron")
    ChainDepStateShelley _ ->
        Left (UnsupportedSnapshotEra snapshotFilePath "Shelley")
    ChainDepStateAllegra _ ->
        Left (UnsupportedSnapshotEra snapshotFilePath "Allegra")
    ChainDepStateMary _ ->
        Left (UnsupportedSnapshotEra snapshotFilePath "Mary")
    ChainDepStateAlonzo _ ->
        Left (UnsupportedSnapshotEra snapshotFilePath "Alonzo")
    ChainDepStateBabbage _ ->
        Left (UnsupportedSnapshotEra snapshotFilePath "Babbage")
    ChainDepStateConway praosState ->
        Right praosState

extractTipSlot :: WithOrigin (AnnTip blk) -> Maybe Word64
extractTipSlot = \case
    Origin ->
        Nothing
    At annTip ->
        Just (unSlotNo (annTipSlotNo annTip))

cardanoCodecConfig :: CardanoCodecConfig StandardCrypto
cardanoCodecConfig =
    CardanoCodecConfig
        (ByronCodecConfig $ EpochSlots 21600)
        ShelleyCodecConfig
        ShelleyCodecConfig
        ShelleyCodecConfig
        ShelleyCodecConfig
        ShelleyCodecConfig
        ShelleyCodecConfig

toFsPath :: FilePath -> FsPath
toFsPath absolutePath =
    mkFsPath pathSegments
  where
    pathSegments =
        filter (not . null) $
            splitDirectories (makeRelative rootFilePath (normalise absolutePath))

rootMountPoint :: MountPoint
rootMountPoint =
    MountPoint rootFilePath

rootFilePath :: FilePath
rootFilePath =
    "/"
