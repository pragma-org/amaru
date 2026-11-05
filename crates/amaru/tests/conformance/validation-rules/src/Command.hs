module Command
    ( runCommandLine
    ) where

import Relude

import qualified Command.ValidatePhaseOne as ValidatePhaseOne
import qualified Command.ValidateMaths as ValidateMaths
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

data Command
    = ValidatePhaseOne ValidatePhaseOne.Options
    | ValidateMaths ValidateMaths.Options

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
    ValidateMaths options ->
        runExceptT (ValidateMaths.run options)

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
        <> command
            "validate-maths"
            ( info
                (ValidateMaths <$> ValidateMaths.optionsParser)
                (progDesc "Validate Cardano fixed-point and VRF leader-threshold vectors")
            )
