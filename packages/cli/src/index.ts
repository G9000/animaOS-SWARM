#!/usr/bin/env node
import { Command } from "commander"
import { runCommand } from "./commands/run.js"
import { chatCommand } from "./commands/chat.js"
import { createCommand } from "./commands/create.js"
import { launchCommand } from "./commands/launch.js"
import { agentsCommand } from "./commands/agents.js"

const program = new Command()

program
	.name("animaos")
	.description("animaOS-SWARM — Command & control your AI agent swarms")
	.version("0.0.1")

program.addCommand(runCommand)
program.addCommand(chatCommand)
program.addCommand(createCommand)
program.addCommand(launchCommand)
program.addCommand(agentsCommand)

program.parse()
