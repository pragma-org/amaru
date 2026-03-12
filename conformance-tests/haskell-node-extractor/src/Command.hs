{-# LANGUAGE NamedFieldPuns #-}

module Command
    ( runCommandLine
    ) where

import Relude

import Command.ExtractSnapshot
    ( ExtractSnapshotOptions (..)
    , extractSnapshotOptionsParser
    )
import Data.Aeson
    ( ToJSON
    )
import Data.Aeson.Encode.Pretty
    ( encodePretty
    )
import Data.NetworkName
    ( networkNameToNetwork
    )
import Error
    ( AppError
    , renderAppError
    )
import Genesis
    ( networkToGenesis
    )
import Options.Applicative
    ( Parser
    , ParserInfo
    , command
    , execParser
    , fullDesc
    , helper
    , hsubparser
    , info
    , progDesc
    )
import Query.DelegateRepresentatives
    ( delegateRepresentativesOutputPath
    , queryDelegateRepresentatives
    )
import Query.Nonces
    ( noncesOutputPath
    , queryNonces
    )
import Query.Pots
    ( potsOutputPath
    , queryPots
    )
import Query.Pools
    ( poolsOutputPath
    , queryPools
    )
import Query.RewardsProvenance
    ( queryRewardsProvenance
    , rewardsProvenanceOutputPath
    )
import Snapshot
    ( LoadedSnapshot (..)
    , loadSnapshot
    )
import System.Directory
    ( createDirectoryIfMissing
    )
import System.FilePath
    ( takeDirectory
    )

data Command
    = ExtractSnapshot ExtractSnapshotOptions

runCommandLine :: IO ()
runCommandLine = do
    selectedCommand <- execParser commandParserInfo
    result <- runExceptT (runCommand selectedCommand)

    case result of
        Left appError -> do
            putTextLn (renderAppError appError)
            exitFailure
        Right () ->
            pure ()

runCommand :: Command -> ExceptT AppError IO ()
runCommand = \case
    ExtractSnapshot options ->
        processSnapshot options

processSnapshot :: ExtractSnapshotOptions -> ExceptT AppError IO ()
processSnapshot options@ExtractSnapshotOptions{networkName} = do
    loadedSnapshot <- loadSnapshot options
    let genesis = networkToGenesis networkName
    let network = networkNameToNetwork networkName
    let epochNumber = loadedSnapshotEpochNumber loadedSnapshot
    let tipSlot = loadedSnapshotTipSlot loadedSnapshot
    let pots = queryPots (loadedSnapshotState loadedSnapshot)
    let potsPath = potsOutputPath epochNumber
    let nonces = queryNonces (loadedSnapshotPraosState loadedSnapshot)
    let noncesPath = noncesOutputPath epochNumber
    let delegateRepresentatives = queryDelegateRepresentatives (loadedSnapshotState loadedSnapshot)
    let delegateRepresentativesPath = delegateRepresentativesOutputPath epochNumber
    let rewardsProvenance = queryRewardsProvenance genesis (loadedSnapshotState loadedSnapshot)
    let rewardsProvenancePath = rewardsProvenanceOutputPath epochNumber
    let pools = queryPools network (loadedSnapshotState loadedSnapshot)
    let poolsPath = poolsOutputPath epochNumber

    liftIO $ do
        putTextLn
            ( "Processing ledger state(epoch = "
                <> show epochNumber
                <> ", slot = "
                <> show tipSlot
                <> ")"
            )
        writeJsonOutput potsPath pots
        writeJsonOutput noncesPath nonces
        writeJsonOutput delegateRepresentativesPath delegateRepresentatives
        writeJsonOutput rewardsProvenancePath rewardsProvenance
        writeJsonOutput poolsPath pools

writeJsonOutput :: ToJSON a => FilePath -> a -> IO ()
writeJsonOutput outputPath jsonValue = do
    putTextLn ("...extracting " <> toText outputPath)
    createDirectoryIfMissing True (takeDirectory outputPath)
    writeFileLBS outputPath (encodePretty jsonValue)

commandParserInfo :: ParserInfo Command
commandParserInfo =
    info
        (helper <*> commandParser)
        (fullDesc <> progDesc "Read a ledger snapshot and extract its current NewEpochState")

commandParser :: Parser Command
commandParser =
    hsubparser $
        command
            "extract"
            ( info
                (ExtractSnapshot <$> extractSnapshotOptionsParser)
                (progDesc "Read a ledger snapshot from disk")
            )
