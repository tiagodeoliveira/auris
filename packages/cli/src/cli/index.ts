#!/usr/bin/env node
import { Command } from "commander";
import { loginCommand, logoutCommand, whoamiCommand } from "./commands/auth.js";
import {
  getCmd,
  listCmd,
  makeClient,
  momentScreenshotCmd,
  searchCmd,
  transcriptCmd,
} from "./commands/meetings.js";

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

const meetings = program.command("meetings").description("Browse your meetings");
meetings
  .command("list")
  .option("--limit <n>", "max rows", (v) => parseInt(v, 10))
  .option("--json", "raw JSON")
  .action(async (o) => console.log(await listCmd(makeClient(), o)));
meetings
  .command("search")
  .option("--query <q>")
  .option("--project <p>")
  .option("--since <d>")
  .option("--until <d>")
  .option("--limit <n>", "max rows", (v) => parseInt(v, 10))
  .option("--json", "raw JSON")
  .action(async (o) => console.log(await searchCmd(makeClient(), o)));
meetings
  .command("get <id>")
  .option("--json", "raw JSON")
  .action(async (id, o) => console.log(await getCmd(makeClient(), id, o)));
meetings
  .command("transcript <id>")
  .option("--offset <n>", "start item", (v) => parseInt(v, 10))
  .option("--limit <n>", "max items", (v) => parseInt(v, 10))
  .option("--json", "raw JSON")
  .action(async (id, o) => console.log(await transcriptCmd(makeClient(), id, o)));
meetings
  .command("moment-screenshot <meetingId> <momentId>")
  .requiredOption("--out <file>", "path to write the PNG")
  .action(async (meetingId, momentId, o) =>
    console.log(await momentScreenshotCmd(makeClient(), meetingId, momentId, o)),
  );

program.parseAsync().catch((e) => {
  console.error((e as Error).message);
  process.exit(1);
});
