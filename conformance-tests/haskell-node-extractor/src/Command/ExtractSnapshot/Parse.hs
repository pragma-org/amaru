module Command.ExtractSnapshot.Parse
    ( Options (..)
    , networkNameParser
    , optionsParser
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
    , bashCompleter
    , completer
    , flag'
    , help
    , long
    , metavar
    , strOption
    )

data Options = Options
    { networkName :: !NetworkName
    , snapshotPath :: !FilePath
    , outputDir :: !FilePath
    }

optionsParser :: Parser Options
optionsParser =
    Options
        <$> networkNameParser
        <*> strOption
            ( long "snapshot"
                <> metavar "PATH"
                <> completer (bashCompleter "directory")
                <> help "Path to the consensus ledger snapshot directory"
            )
        <*> strOption
            ( long "output"
                <> metavar "DIR"
                <> completer (bashCompleter "directory")
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
