module Command.ValidatePhaseOne.Error
  ( Error (..),
    renderError,
  )
where

import Data.Aeson (FromJSON (..), ToJSON (..), Value (..), encode, object, withObject, (.:), (.=))
import Relude

data Error
  = NamedError !Text !Error
  | FixtureReadError !FilePath !Text
  | FixtureDecodeError !FilePath !Text
  | FixtureReferenceError !Text
  | UnsupportedFixture !Text
  | ValidationMismatch !Text !Text
  deriving (Eq, Show)

renderError :: Error -> Text
renderError = decodeUtf8 . encode

instance ToJSON Error where
  toJSON = \case
    NamedError label nestedError ->
      object
        [ "label" .= String label,
          "error" .= toJSON nestedError
        ]
    FixtureReadError path details ->
      object
        [ "type" .= String "FixtureReadError",
          "path" .= String (toText path),
          "details" .= String details
        ]
    FixtureDecodeError path details ->
      object
        [ "type" .= String "FixtureDecodeError",
          "path" .= String (toText path),
          "details" .= String details
        ]
    FixtureReferenceError details ->
      object
        [ "type" .= String "FixtureReferenceError",
          "details" .= String details
        ]
    UnsupportedFixture details ->
      object
        [ "type" .= String "UnsupportedFixture",
          "details" .= String details
        ]
    ValidationMismatch expected actual ->
      object
        [ "type" .= String "ValidationMismatch",
          "expected" .= String expected,
          "actual" .= String actual
        ]

instance FromJSON Error where
  parseJSON = withObject "Error" $ \o ->
    (NamedError <$> o .: "label" <*> o .: "error") <|> do
      errorType <- o .: "type"
      case errorType :: Text of
        "FixtureReadError" ->
          FixtureReadError <$> o .: "path" <*> o .: "details"
        "FixtureDecodeError" ->
          FixtureDecodeError <$> o .: "path" <*> o .: "details"
        "FixtureReferenceError" ->
          FixtureReferenceError <$> o .: "details"
        "UnsupportedFixture" ->
          UnsupportedFixture <$> o .: "details"
        "ValidationMismatch" ->
          ValidationMismatch <$> o .: "expected" <*> o .: "actual"
        other ->
          fail ("unknown error type: " <> toString other)
