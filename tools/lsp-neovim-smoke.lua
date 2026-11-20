-- SPDX-License-Identifier: GPL-3.0-or-later
local server = vim.fs.abspath(assert(arg[1], "expected the path to an unpacked nia-lsp"))
assert(vim.fn.executable(server) == 1, "cannot execute " .. server)

local repo = vim.fs.dirname(vim.fs.dirname(vim.fs.abspath(arg[0])))
local workspace = vim.fn.tempname()
local main = vim.fs.joinpath(workspace, "root.nia")
local source = vim.fs.joinpath(workspace, "source.nia")
local manifest = vim.fs.joinpath(workspace, "nia.toml")
assert(vim.fn.mkdir(workspace) == 1)

local ok, err = xpcall(function()
    assert(vim.fn.writefile({
        "[package]", 'name = "client_smoke"', 'version = "1.0.0"',
        "[library]", 'path = "root.nia"',
    }, manifest) == 0)
    assert(vim.fn.writefile({
        "module source;", "fun value() i32 {", "    source::read()", "}",
    }, main) == 0)
    assert(vim.fn.writefile({ "pub fun read() i32 { 42 }" }, source) == 0)

    vim.opt.runtimepath:prepend(vim.fs.joinpath(repo, "editors", "neovim"))
    vim.cmd.runtime("plugin/nia.lua")
    vim.cmd("filetype plugin on")
    vim.env.PATH = vim.fs.dirname(server) .. (vim.fn.has("win32") == 1 and ";" or ":") .. vim.env.PATH
    assert(vim.uv.fs_realpath(vim.fn.exepath("nia-lsp")) == vim.uv.fs_realpath(server),
        "the release nia-lsp must resolve on PATH")
    vim.lsp.enable("nia")
    vim.cmd.edit(vim.fn.fnameescape(main))
    local buffer = vim.api.nvim_get_current_buf()
    assert(vim.bo.filetype == "nia", "Nia file type was not detected")
    assert(vim.wo.foldexpr == "v:lua.vim.lsp.foldexpr()", "Nia fold expression was not installed")
    assert(vim.bo.commentstring == "// %s", "Nia line comments were not installed")

    assert(vim.wait(10000, function()
        return #vim.lsp.get_clients({ bufnr = buffer, name = "nia" }) == 1
    end), "Nia language server did not attach")
    assert(vim.wait(10000, function()
        return #(vim.lsp.semantic_tokens.get_at_pos(buffer, 2, 12) or {}) > 0
    end), "semantic tokens did not arrive at the call")
    assert(vim.wait(10000, function()
        return vim.lsp.foldexpr(2) == ">1"
    end), "function fold did not arrive")

    vim.api.nvim_win_set_cursor(0, { 3, 12 })
    vim.lsp.buf.rename("fetch", { name = "nia" })
    assert(vim.wait(10000, function()
        return vim.api.nvim_buf_get_lines(buffer, 2, 3, false)[1] == "    source::fetch()"
    end), "Neovim did not apply the rename")
    assert(vim.fn.readfile(main)[3] == "    source::read()", "caller was written before save")
    assert(vim.fn.readfile(source)[1] == "pub fun read() i32 { 42 }", "definition was written before save")
    vim.cmd("silent wall")
    assert(vim.fn.readfile(main)[3] == "    source::fetch()", "caller rename was not saved")
    assert(vim.fn.readfile(source)[1] == "pub fun fetch() i32 { 42 }", "definition rename was not saved")

    vim.api.nvim_win_set_cursor(0, { 3, 12 })
    vim.lsp.buf.definition()
    local definition = vim.uv.fs_realpath(source)
    assert(vim.wait(10000, function()
        return vim.uv.fs_realpath(vim.api.nvim_buf_get_name(0)) == definition
    end), "definition did not resolve after rename")
end, debug.traceback)

for _, path in ipairs({ main, source, manifest, workspace }) do
    vim.fn.delete(path, path == workspace and "d" or "")
end
assert(ok, err)
print("Neovim Nia file type, semantic tokens, folds and cross-file rename passed")
