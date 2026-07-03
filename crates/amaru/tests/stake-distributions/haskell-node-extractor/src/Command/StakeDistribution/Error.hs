module Command.StakeDistribution.Error
    ( Error (..)
    , renderError
    ) where

import Relude

import Error
    ( AppError
    , renderAppError
    )

data Error
    = SnapshotError AppError
    | SnapshotsNotSequential Word64 Word64

renderError :: Error -> Text
renderError = \case
    SnapshotError appError ->
        renderAppError appError
    SnapshotsNotSequential epochNumber nextEpochNumber ->
        "The provided stake-distribution snapshots must be consecutive epochs, but got epoch="
            <> show epochNumber
            <> " and epoch="
            <> show nextEpochNumber
