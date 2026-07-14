module Command.ValidatePhaseOne.Error
    ( Error (..)
    , renderError
    ) where

import Relude

data Error
    = FixtureReadError !FilePath !Text
    | FixtureDecodeError !FilePath !Text
    | FixtureReferenceError !Text
    | UnsupportedFixture !Text
    | ValidationMismatch !Text !Text

renderError :: Error -> Text
renderError = \case
    FixtureReadError path details ->
        "Failed to read fixture at " <> toText path <> ": " <> details
    FixtureDecodeError path details ->
        "Failed to decode fixture at " <> toText path <> ": " <> details
    FixtureReferenceError details ->
        "Failed to resolve fixture references: " <> details
    UnsupportedFixture details ->
        "Unsupported fixture: " <> details
    ValidationMismatch expected actual ->
        "Validation mismatch. Expected "
            <> expected
            <> " but got "
            <> actual
