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
    | ExpectedFailure !Text

instance FromJSON Expected where
    parseJSON = \case
        String "Pass" ->
            pure ExpectedPass
        Object objectValue
            | KeyMap.member "predicate" objectValue ->
                ExpectedFailure <$> objectValue .: "predicate"
        otherValue ->
            fail ("Expected \"Pass\" or { predicate: ... }, but got " <> show otherValue)

expectedPredicate :: Expected -> Maybe Text
expectedPredicate = \case
    ExpectedPass ->
        Nothing
    ExpectedFailure predicateName ->
        Just predicateName
