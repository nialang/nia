const vscode = require("vscode");
const { spawn } = require("node:child_process");
const { createInterface } = require("node:readline");

const types = [
    "keyword", "type", "primitive", "interface", "typeParameter", "namespace",
    "function", "method", "variable", "parameter", "property", "enumMember",
    "constant", "decorator", "number", "string", "comment", "operator",
    "propagation", "dereference",
];
const tokenIndices = new Map(types.map((type, index) => [type, index]));
const legend = new vscode.SemanticTokensLegend(types);

function activate(context) {
    const documents = new Map();
    const queue = new Map();
    const log = vscode.window.createOutputChannel("Nia");
    let worker;
    let active;
    let id = 0;
    let executable;
    let failed = false;

    function stop(process, error) {
        if (worker !== process) return;
        worker = undefined;
        process.kill();
        active?.resolve(undefined);
        active = undefined;
        for (const item of queue.values()) item.resolve(undefined);
        queue.clear();
        documents.clear();
        if (error) {
            failed = true;
            log.appendLine(String(error));
            vscode.window.showWarningMessage(
                "Nia could not run nia-editor. Install the helper or set nia.editorPath.",
            );
        }
    }

    function dispatch() {
        if (active || failed || !queue.size) return;
        if (!worker) {
            const process = spawn(executable, [], { windowsHide: true });
            worker = process;
            process.on("error", (error) => stop(process, error));
            process.on("close", (code) => stop(process, `nia-editor exited with status ${code}`));
            process.stdin.on("error", (error) => stop(process, error));
            process.stderr.on("data", (chunk) => log.append(String(chunk)));
            createInterface({ input: process.stdout }).on("line", (line) => {
                if (worker !== process || !active) return;
                try {
                    const response = JSON.parse(line);
                    if (response.id !== active.id) throw new Error("Unexpected response ID");
                    if (response.error) log.appendLine(response.error);
                    active.resolve(response.result);
                    active = undefined;
                    dispatch();
                } catch (error) {
                    stop(process, error);
                }
            });
        }
        const [key, item] = queue.entries().next().value;
        queue.delete(key);
        active = item;
        worker.stdin.write(JSON.stringify({ id: item.id, text: item.document.getText() }) + "\n");
    }

    function read(document) {
        const path = vscode.workspace.getConfiguration("nia").get("editorPath");
        if (path !== executable) {
            if (worker) stop(worker);
            executable = path;
            failed = false;
        }
        if (failed) return Promise.resolve(undefined);
        const key = document.uri.toString();
        const snapshot = documents.get(key);
        if (snapshot?.version === document.version) return snapshot.result;

        const requestId = ++id;
        const version = document.version;
        const result = new Promise((resolve) => {
            queue.get(key)?.resolve(undefined);
            queue.set(key, { id: requestId, document, resolve });
            dispatch();
        }).then((data) => document.version === version && documents.get(key)?.result === result
            ? data
            : undefined);
        documents.set(key, { version, result });
        return result;
    }

    context.subscriptions.push(
        log,
        { dispose: () => { if (worker) stop(worker); } },
        vscode.workspace.onDidCloseTextDocument((document) => {
            const key = document.uri.toString();
            documents.delete(key);
            queue.get(key)?.resolve(undefined);
            queue.delete(key);
        }),
        vscode.languages.registerDocumentSemanticTokensProvider("nia", {
            async provideDocumentSemanticTokens(document, cancellation) {
                const data = await read(document);
                if (!data || cancellation.isCancellationRequested) return undefined;
                const tokens = new vscode.SemanticTokensBuilder();
                for (const [line, , , column, length, type] of data.tokens) {
                    const index = tokenIndices.get(type);
                    if (index !== undefined) tokens.push(line, column, length, index, 0);
                }
                return tokens.build();
            },
        }, legend),
        vscode.languages.registerFoldingRangeProvider("nia", {
            async provideFoldingRanges(document, _context, cancellation) {
                const data = await read(document);
                if (!data || cancellation.isCancellationRequested) return undefined;
                return data.folds.map(([first, last, kind]) => new vscode.FoldingRange(
                    first, last, kind === "imports" ? vscode.FoldingRangeKind.Imports
                        : vscode.FoldingRangeKind.Region,
                ));
            },
        }),
    );
}

module.exports = { activate };
