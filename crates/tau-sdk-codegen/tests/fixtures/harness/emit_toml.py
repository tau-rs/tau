import sys

sys.stdout.write(
    'packages = ["anthropic"]\n'
    '\n'
    '[project]\n'
    'name = "harness"\n'
    '\n'
    '[models.haiku]\n'
    'backend = "anthropic"\n'
    'model = "claude-haiku-4-5"\n'
    '\n'
    '[agents.fast]\n'
    'display_name = "Fast"\n'
    'package = "research@^0.1"\n'
    'model = "haiku"\n'
)
