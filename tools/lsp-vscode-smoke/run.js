// SPDX-License-Identifier: GPL-3.0-or-later
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vscode = require("vscode");

exports.run = async () => {
    const root = vscode.workspace.workspaceFolders[0].uri;
    const at = (...parts) => vscode.Uri.joinPath(root, ...parts);
    const main = at("root.nia");
    const old = at("source.nia");
    const moved = at("destination.nia");
    const fromTree = at("source");
    const toTree = at("destination");
    const doc = await vscode.workspace.openTextDocument(main);
    await vscode.window.showTextDocument(doc);
    const extension = vscode.extensions.getExtension("nialang.nia");
    try {
        assert.ok(extension);
        await extension.activate();
        assert.equal(doc.languageId, "nia");
        const definitions = async () => vscode.commands.executeCommand(
            "vscode.executeDefinitionProvider", main, new vscode.Position(2, 21),
        );
        const original = await definitions();
        assert.equal(original[0]?.uri.fsPath, at("source", "nested.nia").fsPath);
        const folds = await vscode.commands.executeCommand("vscode.executeFoldingRangeProvider", main);
        assert.ok(folds.some((fold) => fold.start === 1 && fold.end >= 2));
        const tokens = await vscode.commands.executeCommand("vscode.provideDocumentSemanticTokens", main);
        assert.ok(tokens.data.length > 0);
        const expectedIcon = fs.readFileSync(process.env.NIA_SMOKE_ICON);
        assert.deepEqual(fs.readFileSync(path.join(extension.extensionPath, "assets", "nia-icon.svg")), expectedIcon);
        assert.equal(vscode.workspace.getConfiguration("workbench").get("iconTheme"), "nia");

        const files = new vscode.WorkspaceEdit();
        files.renameFile(old, moved);
        files.renameFile(fromTree, toTree);
        assert.ok(await vscode.workspace.applyEdit(files));
        assert.ok(fs.existsSync(at("destination", "nested.nia").fsPath));
        assert.ok(!fs.existsSync(old.fsPath));
        assert.match(doc.getText(), /^module destination;/m);
        assert.match(doc.getText(), /destination::nested::read/);
        assert.ok(await doc.save());
        assert.match(fs.readFileSync(main.fsPath, "utf8"), /destination::nested::read/);
        const result = await vscode.commands.executeCommand(
            "vscode.executeDefinitionProvider", main, new vscode.Position(2, 26),
        );
        assert.equal(result[0]?.uri.fsPath, at("destination", "nested.nia").fsPath);
        fs.writeFileSync(process.env.NIA_SMOKE_RESULT, "VS Code module move, semantic tokens, folds and brand asset passed\n");
    } catch (error) {
        fs.writeFileSync(process.env.NIA_SMOKE_RESULT, String(error.stack ?? error));
        throw error;
    }
};
