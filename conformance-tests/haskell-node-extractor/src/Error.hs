module Error
    ( AppError (..)
    , renderAppError
    ) where

import Relude

import Ouroboros.Consensus.Util.CBOR
    ( ReadIncrementalErr
    )

data AppError
    = SnapshotReadError !FilePath !ReadIncrementalErr
    | SnapshotHasNoTip !FilePath
    | InvalidByronEpochSlots !Word64
    | UnsupportedSnapshotEra !FilePath !Text

renderAppError :: AppError -> Text
renderAppError = \case
    SnapshotReadError snapshotFilePath readError ->
        "Failed to read snapshot at "
            <> toText snapshotFilePath
            <> ": "
            <> show readError
    SnapshotHasNoTip snapshotFilePath ->
        "Snapshot at "
            <> toText snapshotFilePath
            <> " does not have a tip slot."
    InvalidByronEpochSlots epochSlots ->
        "Failed to construct Byron epoch slots from "
            <> show epochSlots
            <> "."
    UnsupportedSnapshotEra snapshotFilePath actualEra ->
        "Snapshot at "
            <> toText snapshotFilePath
            <> " is a "
            <> actualEra
            <> " snapshot. Only Conway snapshots are supported."
