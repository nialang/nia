// SPDX-License-Identifier: GPL-3.0-or-later
const vscode = require("vscode");
const { LanguageClient } = require("vscode-languageclient/node");

let client;

async function activate() {
    const config = vscode.workspace.getConfiguration("nia.server");
    const root = config.get("resourceRoot");
    const args = root ? ["--resource-root", root] : [];
    client = new LanguageClient(
        "nia",
        "Nia Language Server",
        { command: config.get("path", "nia-lsp"), args, options: { windowsHide: true } },
        { documentSelector: [{ scheme: "file", language: "nia" }] },
    );
    await client.start();
}

function deactivate() {
    return client?.stop();
}

module.exports = { activate, deactivate };
