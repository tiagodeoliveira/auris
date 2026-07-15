#!/usr/bin/env node
import { Command } from "commander";
import { loginCommand, logoutCommand, whoamiCommand } from "./commands/auth.js";

const program = new Command();
program.name("auris").description("Auris CLI — your meetings from the terminal");

program
  .command("login")
  .description("Log in via Auth0 device flow")
  .action(async () => {
    await loginCommand();
  });
program
  .command("logout")
  .description("Clear stored credentials")
  .action(async () => {
    await logoutCommand();
  });
program
  .command("whoami")
  .description("Show the logged-in identity")
  .action(async () => {
    console.log(await whoamiCommand());
  });

program.parseAsync().catch((e) => {
  console.error((e as Error).message);
  process.exit(1);
});
