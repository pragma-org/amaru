module Command.ExtractSnapshot.Error
    ( Error (..)
    , renderError
    ) where

import Relude

import Error
    ( AppError
    , renderAppError
    )

newtype Error
    = SnapshotError AppError

renderError :: Error -> Text
renderError = \case
    SnapshotError appError ->
        renderAppError appError
