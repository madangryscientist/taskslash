# Security

## Reporting a vulnerability

Report it privately, not in a public issue: use GitHub's private vulnerability reporting,
**Security > Report a vulnerability** on the repository (https://github.com/madangryscientist/slashproduct).
If you cannot use GitHub, email hello@hack-cove.com. Only the maintainers can read the report. Say
what you found, the version or commit, and the steps to reproduce it; a proof of concept helps.

This is a one-maintainer project, so these are targets, not guarantees: an acknowledgement within 5
working days, and a fix or a plan within 30 days for anything in scope. We will credit you in the
advisory unless you ask us not to. Please give us that time before you disclose, and do not test
against someone else's deployment or data.

## What is in scope

The code in this repository: the engine, the command line and, where there is one, the web app.

Out of scope: a deployment you configured weakly, a vulnerability in a dependency that does not
affect SlashProduct (report it upstream), and findings that need a stolen token or a machine you
already control.

## The threat model, short

The code is public, so assume an attacker has read it. Nothing here depends on the code being
secret: every secret is read from the environment, never from the tree. This section grows as the
product does.
