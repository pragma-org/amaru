module Data.Fixture.SharedDocument
    ( SharedDocument (..)
    , resolveSharedDocument
    ) where

import Relude

import Command.ValidatePhaseOne.Error
    ( Error (..)
    )
import Data.Aeson
    ( FromJSON (parseJSON)
    , Object
    , Value (..)
    , eitherDecodeFileStrict'
    , withObject
    , (.:)
    , (.:?)
    )
import qualified Data.Aeson.KeyMap as KeyMap
import System.FilePath
    ( (</>)
    )

data SharedDocument
    = InlineDocument !Value
    | ReferencedDocument !FilePath !(Maybe Object)

instance FromJSON SharedDocument where
    parseJSON value =
        case value of
            Object objectValue
                | KeyMap.member "$ref" objectValue ->
                    withObject "SharedDocument" parseReference value
            _ ->
                pure (InlineDocument value)
      where
        parseReference objectValue =
            ReferencedDocument
                <$> objectValue .: "$ref"
                <*> objectValue .:? "$override"

resolveSharedDocument :: FilePath -> Text -> SharedDocument -> ExceptT Error IO Value
resolveSharedDocument fixtureRoot fieldName = \case
    InlineDocument value ->
        pure value
    ReferencedDocument relativePath maybeOverride -> do
        referencedValue <- ExceptT $
            first (FixtureReadError (fixtureRoot </> relativePath) . toText)
                <$> liftIO (eitherDecodeFileStrict' (fixtureRoot </> relativePath))
        case maybeOverride of
            Nothing ->
                pure referencedValue
            Just overrideValue ->
                case referencedValue of
                    Object referencedObject ->
                        pure (Object (KeyMap.union overrideValue referencedObject))
                    _ ->
                        hoistEither
                            ( Left
                                ( FixtureReferenceError
                                    ( "field \""
                                        <> fieldName
                                        <> "\" uses $override, but the referenced value is not an object"
                                    )
                                )
                            )
