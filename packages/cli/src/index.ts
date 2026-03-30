#!/usr/bin/env node
import { Command } from "commander"
import { runCommand } from "./commands/run.js"
import { chatCommand } from "./commands/chat.js"

const program = new Command()

program
	.name("animaos-swarm")
	.description("AnimaOS Kit — Task Agent Swarm Framework")
	.version("0.0.1")

program.addCommand(runCommand)
program.addCommand(chatCommand)

program.parse()
