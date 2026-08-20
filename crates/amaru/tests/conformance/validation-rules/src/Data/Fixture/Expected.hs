module Data.Fixture.Expected
    ( Expected (..)
    , expectedPredicate
    ) where

import Relude

import Data.Aeson
    ( FromJSON (parseJSON)
    , Value (Object, String)
    , (.:)
    )
import qualified Data.Aeson.KeyMap as KeyMap

data Expected
    = ExpectedPass
    | ExpectedDecodingFailure
    | ExpectedFailure !Text

instance FromJSON Expected where
    parseJSON = \case
        String "Pass" ->
            pure ExpectedPass
        Object objectValue
            | KeyMap.member "decoding_failure" objectValue ->
                pure ExpectedDecodingFailure
            | KeyMap.member "predicate" objectValue ->
                ExpectedFailure <$> objectValue .: "predicate"
        otherValue ->
            fail ("Expected \"Pass\", { decodingFailure: ... } or { predicate: ... }, but got " <> show otherValue)

expectedPredicate :: Expected -> Maybe Text
expectedPredicate = \case
    ExpectedPass ->
        Nothing
    ExpectedDecodingFailure ->
        Nothing
    ExpectedFailure predicateName ->
        Just predicateName
