{-# LANGUAGE DataKinds #-}
{-# LANGUAGE DeriveGeneric #-}
{-# LANGUAGE DuplicateRecordFields #-}

module Data.Fixture.TestCase
    ( TestCase (..)
    , TestCaseDocument (..)
    , loadTestCase
    , testCaseLabel
    ) where

import Relude

import Cardano.Ledger.Api.PParams
    ( ppProtocolVersionL
    )
import Cardano.Ledger.BaseTypes
    ( ProtVer (pvMajor)
    )
import Cardano.Ledger.Binary
    ( decCBOR
    , decodeFullAnnotator
    )
import Cardano.Ledger.Binary.Plain
    ( withHexText
    )
import Cardano.Ledger.Conway
    ( ConwayEra
    )
import Cardano.Ledger.Core
    ( Tx
    , TxLevel (TopTx)
    )
import Command.ValidatePhaseOne.Error
    ( Error (..)
    )
import Data.Aeson
    ( FromJSON
    , parseJSON
    , eitherDecodeFileStrict'
    )
import Data.Aeson.Types
    ( parseEither
    )
import Data.Fixture.Common
    ( showText
    )
import Data.Fixture.EraHistory
    ( EraHistory
    )
import Data.Fixture.Expected
    ( Expected
    )
import Data.Fixture.InitialState
    ( InitialState
    )
import Data.Fixture.Network
    ( FixtureNetwork
    )
import Data.Fixture.Point
    ( Point
    )
import Data.Fixture.ProtocolParameters
    ( ProtocolParameters
    , pparams
    , protocolParametersFromJson
    )
import Data.Fixture.SharedDocument
    ( SharedDocument
    , resolveSharedDocument
    )
import qualified Data.ByteString.Lazy as LazyByteString
import Lens.Micro
    ( (^.)
    )
import System.FilePath
    ( dropExtension
    , takeDirectory
    , takeFileName
    )

data TestCaseDocument = TestCaseDocument
    { title :: !(Maybe Text)
    , description :: !(Maybe Text)
    , network :: !FixtureNetwork
    , eraHistory :: !SharedDocument
    , protocolParameters :: !SharedDocument
    , initialState :: !InitialState
    , point :: !Point
    , transaction :: !Text
    , expected :: !Expected
    }
    deriving (Generic)

instance FromJSON TestCaseDocument

data TestCase = TestCase
    { sourcePath :: !FilePath
    , title :: !(Maybe Text)
    , description :: !(Maybe Text)
    , network :: !FixtureNetwork
    , eraHistory :: !EraHistory
    , protocolParameters :: !ProtocolParameters
    , initialState :: !InitialState
    , point :: !Point
    , transaction :: !(Either Error (Tx TopTx ConwayEra))
    , expected :: !Expected
    }

loadTestCase :: FilePath -> ExceptT Error IO TestCase
loadTestCase testCasePath = do
    TestCaseDocument
        { title = documentTitle
        , description = documentDescription
        , network = documentNetwork
        , eraHistory = documentEraHistory
        , protocolParameters = documentProtocolParameters
        , initialState = documentInitialState
        , point = documentPoint
        , transaction = documentTransaction
        , expected = documentExpected
        } <-
        ExceptT (first (FixtureReadError testCasePath . toText) <$> liftIO (eitherDecodeFileStrict' testCasePath))
    fixtureRoot <- hoistEither (findFixtureRoot testCasePath)
    resolvedEraHistoryValue <- resolveSharedDocument fixtureRoot "eraHistory" documentEraHistory
    resolvedProtocolParametersValue <- resolveSharedDocument fixtureRoot "protocolParameters" documentProtocolParameters
    resolvedEraHistory <- hoistEither (first (FixtureDecodeError testCasePath . toText) (parseEither parseJSON resolvedEraHistoryValue))
    resolvedProtocolParameters <-
        hoistEither
            ( first
                (FixtureDecodeError testCasePath . toText)
                (parseEither protocolParametersFromJson resolvedProtocolParametersValue)
            )
    let decodedTransaction = decodeTransaction resolvedProtocolParameters documentTransaction
    pure
        TestCase
            { sourcePath = testCasePath
            , title = documentTitle
            , description = documentDescription
            , network = documentNetwork
            , eraHistory = resolvedEraHistory
            , protocolParameters = resolvedProtocolParameters
            , initialState = documentInitialState
            , point = documentPoint
            , transaction = decodedTransaction
            , expected = documentExpected
            }

testCaseLabel :: TestCase -> Text
testCaseLabel TestCase{title = Just fixtureTitle} =
    fixtureTitle
testCaseLabel TestCase{sourcePath} =
    toText (dropExtension (takeFileName sourcePath))

findFixtureRoot :: FilePath -> Either Error FilePath
findFixtureRoot fixturePath =
    go (takeDirectory fixturePath)
  where
    go directory
        | takeFileName directory == "phase-one" =
            Right directory
        | parent == directory =
            Left (FixtureReferenceError ("could not locate the phase-one fixture root from " <> toText fixturePath))
        | otherwise =
            go parent
      where
        parent = takeDirectory directory

decodeTransaction :: ProtocolParameters -> Text -> Either Error (Tx TopTx ConwayEra)
decodeTransaction protocolParametersValue hexText =
    first (UnsupportedFixture . ("cannot decode transaction CBOR: " <>) . showText) $
        withHexText
            ( \bytes ->
                decodeFullAnnotator
                    (pvMajor (pparams protocolParametersValue ^. ppProtocolVersionL))
                    "Tx"
                    decCBOR
                    (LazyByteString.fromStrict bytes)
            )
            hexText
