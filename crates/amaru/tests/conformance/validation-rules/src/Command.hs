module Command
    ( runCommandLine
    ) where

import Relude

import qualified Command.ValidatePhaseOne as ValidatePhaseOne
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

newtype Command
    = ValidatePhaseOne ValidatePhaseOne.Options

runCommandLine :: IO ()
runCommandLine = do
    selectedCommand <- execParser commandParserInfo
    result <- runCommand selectedCommand

    case result of
        Left errorMessage -> do
            putTextLn errorMessage
            exitFailure
        Right () ->
            pure ()

runCommand :: Command -> IO (Either Text ())
runCommand = \case
    ValidatePhaseOne options ->
        first ValidatePhaseOne.renderError <$> runExceptT (ValidatePhaseOne.run options)

commandParserInfo :: ParserInfo Command
commandParserInfo =
    info
        (helper <*> commandParser)
        (fullDesc <> progDesc "Run conformance tests against the Haskell reference implementation")

commandParser :: Parser Command
commandParser =
    hsubparser $
        command
            "validate-phase-one"
            ( info
                (ValidatePhaseOne <$> ValidatePhaseOne.optionsParser)
                (progDesc "Validate a transaction conformance fixture with the Haskell ledger rules")
            )
