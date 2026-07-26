module Command.ValidatePhaseOne.ErrorSpec (spec) where

import Command.ValidatePhaseOne.Error (Error (..), renderError)
import Data.Aeson (Value (..), decode, encode, object, toJSON, (.=))
import Relude
import Test.Hspec
import Test.Hspec.QuickCheck (prop)
import Test.QuickCheck (Gen, arbitrary, forAll, oneof, sized, (===))

spec :: Spec
spec = do
  describe "encoding" $ do
    it "encodes FixtureReadError" $
      toJSON (FixtureReadError "fixtures/test.json" "file not found")
        `shouldBe` object
          [ "type" .= String "FixtureReadError",
            "path" .= String "fixtures/test.json",
            "details" .= String "file not found"
          ]

    it "encodes FixtureDecodeError" $
      toJSON (FixtureDecodeError "fixtures/test.json" "invalid JSON")
        `shouldBe` object
          [ "type" .= String "FixtureDecodeError",
            "path" .= String "fixtures/test.json",
            "details" .= String "invalid JSON"
          ]

    it "encodes FixtureReferenceError" $
      toJSON (FixtureReferenceError "missing reference")
        `shouldBe` object
          [ "type" .= String "FixtureReferenceError",
            "details" .= String "missing reference"
          ]

    it "encodes UnsupportedFixture" $
      toJSON (UnsupportedFixture "unknown era")
        `shouldBe` object
          [ "type" .= String "UnsupportedFixture",
            "details" .= String "unknown era"
          ]

    it "encodes ValidationMismatch" $
      toJSON (ValidationMismatch "Valid" "Invalid")
        `shouldBe` object
          [ "type" .= String "ValidationMismatch",
            "expected" .= String "Valid",
            "actual" .= String "Invalid"
          ]

    it "encodes NamedError by nesting the wrapped error" $
      toJSON (NamedError "test-1" (NamedError "phase-1" (UnsupportedFixture "unknown era")))
        `shouldBe` object
          [ "label" .= String "test-1",
            "error"
              .= object
                [ "label" .= String "phase-1",
                  "error"
                    .= object
                      [ "type" .= String "UnsupportedFixture",
                        "details" .= String "unknown era"
                      ]
                ]
          ]

  describe "decoding" $
    prop "roundtrips any error through encoding and decoding" $
      forAll genError $ \err ->
        decode (encode err) === Just err

  describe "renderError" $
    prop "renders any error as its JSON encoding" $
      forAll genError $ \err ->
        decode (encodeUtf8 (renderError err)) === Just err

genError :: Gen Error
genError = sized go
  where
    go n =
      oneof $
        [ FixtureReadError <$> arbitrary <*> genText,
          FixtureDecodeError <$> arbitrary <*> genText,
          FixtureReferenceError <$> genText,
          UnsupportedFixture <$> genText,
          ValidationMismatch <$> genText <*> genText
        ]
          <> [NamedError <$> genText <*> go (n `div` 2) | n > 0]

    genText = toText <$> (arbitrary :: Gen String)
