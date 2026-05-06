---
description: Work with Grapha symbol annotations and sync
---
Use Grapha's annotation commands based on the requested action:

- To add or replace a symbol note: `grapha symbol annotate "<symbol>" "<annotation>" --by codex`
- To inspect one symbol note: `grapha symbol annotation "<symbol>"`
- To list local annotation records and project/branch identity: `grapha annotation list -p .`
- To deploy the local LAN annotation service: `grapha annotation serve -p . --port 8080`
- To sync with another local Grapha annotation service: `grapha annotation sync --server http://HOST:8080 -p .`

After syncing, use `grapha symbol context "<symbol>" --fields annotation` or `grapha symbol search "<query>" --fields annotation` to verify that the expected knowledge is available.
