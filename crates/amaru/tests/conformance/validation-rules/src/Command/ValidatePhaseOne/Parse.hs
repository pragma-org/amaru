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

newtype Options = Options { target :: Target }
  deriving (Eq, Show)

data Target
    = SingleTestCase !FilePath
    | TestCaseDirectory !FilePath
  deriving (Eq, Show)

optionsParser :: Parser Options
optionsParser =
    Options <$> (singleTestCase <|> testCaseDirectory)
  where
    singleTestCase =
        SingleTestCase
            <$> strOption
                ( long "test-case"
                    <> metavar "PATH"
                    <> completer (bashCompleter "file")
                    <> help "Path to a phase-one conformance fixture"
                )
    testCaseDirectory =
        TestCaseDirectory
            <$> strOption
                ( long "test-directory"
                    <> metavar "PATH"
                    <> completer (bashCompleter "directory")
                    <> help "Path to a directory of phase-one conformance fixtures, validated recursively with one JSON result per line"
                )
