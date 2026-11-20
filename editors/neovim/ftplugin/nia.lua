-- SPDX-License-Identifier: GPL-3.0-or-later
vim.opt_local.foldmethod = "expr"
vim.opt_local.commentstring = "// %s"
vim.opt_local.foldexpr = "v:lua.vim.lsp.foldexpr()"
local undo = vim.b.undo_ftplugin
vim.b.undo_ftplugin = (undo and (undo .. " | ") or "") .. "setlocal foldmethod< foldexpr< commentstring<"
