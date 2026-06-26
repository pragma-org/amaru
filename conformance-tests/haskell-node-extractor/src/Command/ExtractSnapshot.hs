module Command.ExtractSnapshot
    ( ExtractSnapshotOptions (..)
    , extractSnapshotOptionsParser
    ) where

import Relude

import Data.NetworkName
    ( NetworkName
        ( Mainnet
        , Preprod
        , Preview
        )
    )
import Options.Applicative
    ( Parser
    , flag'
    , help
    , long
    , metavar
    , strOption
    )

data ExtractSnapshotOptions = ExtractSnapshotOptions
    { networkName :: !NetworkName
    , snapshotPath :: !FilePath
    , outputDir :: !FilePath
    }

extractSnapshotOptionsParser :: Parser ExtractSnapshotOptions
extractSnapshotOptionsParser =
    ExtractSnapshotOptions
        <$> networkNameParser
        <*> strOption
            ( long "snapshot"
                <> metavar "PATH"
                <> help "Path to the consensus ledger snapshot directory"
            )
        <*> strOption
            ( long "output"
                <> metavar "DIR"
                <> help "Directory where extracted JSON files will be written (e.g. ./data)"
            )

networkNameParser :: Parser NetworkName
networkNameParser =
    asum
        [ flag'
            Mainnet
            ( long "mainnet"
                <> help "Use mainnet genesis parameters"
            )
        , flag'
            Preprod
            ( long "preprod"
                <> help "Use preprod genesis parameters"
            )
        , flag'
            Preview
            ( long "preview"
                <> help "Use preview genesis parameters"
            )
        ]
