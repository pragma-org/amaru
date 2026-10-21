module Command.ValidatePhaseOne.Parse
    ( Options (..)
    , Target (..)
    , optionsParser
    ) where

import Relude

import Options.Applicative
    ( Parser
    , bashCompleter
    , completer
    , help
    , long
    , metavar
    , strOption
    )

data Options = Options
    { target :: Target
    , jsonReport :: Maybe FilePath
    }
  deriving (Eq, Show)

data Target
    = SingleTestCase !FilePath
    | TestCaseDirectory !FilePath
  deriving (Eq, Show)

optionsParser :: Parser Options
optionsParser =
    Options
        <$> (singleTestCase <|> testCaseDirectory)
        <*> optional
            ( strOption
                ( long "json-report"
                    <> metavar "PATH"
                    <> completer (bashCompleter "file")
                    <> help "Write a complete ledger-rules report as JSON"
                )
            )
  where
    singleTestCase =
        SingleTestCase
            <$> strOption
                ( long "test-case"
                    <> metavar "PATH"
                    <> completer (bashCompleter "file")
                    <> help "Path to a transaction conformance fixture"
                )
    testCaseDirectory =
        TestCaseDirectory
            <$> strOption
                ( long "test-directory"
                    <> metavar "PATH"
                    <> completer (bashCompleter "directory")
                    <> help "Path to a directory of transaction conformance fixtures, validated recursively with one JSON result per line"
                )
