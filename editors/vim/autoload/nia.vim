let s:roles = {
      \ 'keyword': 'Keyword', 'type': 'Type', 'primitive': 'Type',
      \ 'interface': 'Type', 'typeParameter': 'Typedef', 'namespace': 'Include',
      \ 'function': 'Function', 'method': 'Function', 'variable': 'Identifier',
      \ 'parameter': 'Identifier', 'property': 'Identifier', 'enumMember': 'Constant',
      \ 'constant': 'Constant', 'decorator': 'PreProc', 'number': 'Number',
      \ 'string': 'String', 'comment': 'Comment', 'operator': 'Operator',
      \ 'propagation': 'Special', 'dereference': 'Operator',
      \ }
let s:namespace = has('nvim') ? nvim_create_namespace('nia') : 0
let s:worker = 0
let s:active = {}
let s:queue = {}
let s:next_id = 0
let s:partial = ''

function! nia#attach() abort
  if get(b:, 'nia_attached', 0)
    return
  endif
  if !has('nvim') && !(has('job') && has('channel') && exists('*prop_add_list') && has('timers'))
    echohl WarningMsg
    echom 'Nia requires Vim with jobs, channels, text properties and timers.'
    echohl None
    return
  endif
  for [role, link] in items(s:roles)
    let group = 'Nia_' . role
    execute 'highlight default link ' . group . ' ' . link
    if !has('nvim') && empty(prop_type_get(group))
      call prop_type_add(group, {'highlight': group, 'combine': 0})
    endif
  endfor
  let b:nia_attached = 1
  let b:undo_ftplugin = get(b:, 'undo_ftplugin', '')
        \ . (empty(get(b:, 'undo_ftplugin', '')) ? '' : ' | ')
        \ . 'call nia#detach()'
  if get(g:, 'nia_folding', 1)
    setlocal foldmethod=expr foldexpr=nia#fold(v:lnum)
  endif
  call nia#schedule()
endfunction

function! nia#icons() abort
  let glyph = get(g:, 'nia_file_icon', 'N')
  if exists('g:WebDevIconsUnicodeDecorateFileNodesExtensionSymbols')
    let g:WebDevIconsUnicodeDecorateFileNodesExtensionSymbols.nia = glyph
  endif
  if has('nvim')
    lua if package.loaded['nvim-web-devicons'] then require('nvim-web-devicons').set_icon({ nia = { icon = vim.g.nia_file_icon or 'N', color = '#b8235a', cterm_color = '125', name = 'Nia' } }) end
  endif
endfunction

function! nia#detach() abort
  call nia#forget(bufnr())
  if has('nvim')
    call nvim_buf_clear_namespace(bufnr(), s:namespace, 0, -1)
  else
    call prop_remove({'types': s:groups(), 'all': 1})
  endif
  unlet! b:nia_attached b:nia_folds
  if &l:foldexpr ==# 'nia#fold(v:lnum)'
    setlocal foldmethod< foldexpr<
  endif
endfunction

function! nia#fold(line) abort
  return get(get(b:, 'nia_folds', []), a:line - 1, 0)
endfunction

function! nia#forget(buffer) abort
  let timer = getbufvar(a:buffer, 'nia_timer', -1)
  if timer != -1
    call timer_stop(timer)
  endif
  if has_key(s:queue, a:buffer)
    call remove(s:queue, a:buffer)
  endif
endfunction

function! nia#schedule() abort
  if !get(b:, 'nia_attached', 0)
    return
  endif
  let timer = get(b:, 'nia_timer', -1)
  if timer == -1
    let b:nia_timer = timer_start(16, function('s:enqueue', [bufnr()]))
  endif
endfunction

function! s:enqueue(buffer, timer) abort
  if !bufloaded(a:buffer) || !getbufvar(a:buffer, 'nia_attached', 0)
    return
  endif
  call setbufvar(a:buffer, 'nia_timer', -1)
  let s:queue[a:buffer] = a:buffer
  call s:dispatch()
endfunction

function! s:dispatch() abort
  while empty(s:active) && !empty(s:queue)
    let key = keys(s:queue)[0]
    let buffer = remove(s:queue, key)
    if !bufloaded(buffer) || !getbufvar(buffer, 'nia_attached', 0)
      continue
    endif
    if !s:start()
      return
    endif
    let s:next_id += 1
    let s:active = {'id': s:next_id, 'buffer': buffer, 'tick': getbufvar(buffer, 'changedtick')}
    let request = json_encode({'id': s:next_id, 'text': join(getbufline(buffer, 1, '$'), "\n")}) . "\n"
    if has('nvim')
      call chansend(s:worker, request)
    else
      call ch_sendraw(s:worker, request)
    endif
  endwhile
endfunction

function! s:start() abort
  if s:worker isnot 0
    return 1
  endif
  let executable = get(g:, 'nia_editor_path', 'nia-editor')
  if !executable(executable)
    if !get(s:, 'reported', 0)
      let s:reported = 1
      echohl WarningMsg
      echom 'Nia could not find nia-editor. Set g:nia_editor_path or add it to PATH.'
      echohl None
    endif
    let s:queue = {}
    return 0
  endif
  let s:reported = 0
  if has('nvim')
    let s:worker = jobstart([executable], {
          \ 'on_stdout': function('s:nvim_data'),
          \ 'on_exit': function('s:exited'),
          \ })
    if s:worker <= 0
      let s:worker = 0
    endif
  else
    let s:worker = job_start([executable], {
          \ 'in_mode': 'raw', 'out_mode': 'nl',
          \ 'out_cb': function('s:vim_data'),
          \ 'exit_cb': function('s:exited'),
          \ })
    if job_status(s:worker) !=# 'run'
      let s:worker = 0
    endif
  endif
  return s:worker isnot 0
endfunction

function! s:vim_data(channel, line) abort
  call s:receive(a:line)
endfunction

function! s:nvim_data(job, lines, event) abort
  if a:job != s:worker || empty(a:lines)
    return
  endif
  let lines = copy(a:lines)
  let lines[0] = s:partial . lines[0]
  let s:partial = remove(lines, -1)
  for line in lines
    call s:receive(line)
  endfor
endfunction

function! s:exited(job, code, ...) abort
  if a:job is s:worker
    let s:worker = 0
    let s:active = {}
    let s:partial = ''
    let s:queue = {}
  endif
endfunction

function! s:receive(line) abort
  let response = json_decode(a:line)
  if empty(s:active) || response.id != s:active.id
    return
  endif
  let request = s:active
  let s:active = {}
  let buffer = request.buffer
  if response.result isnot v:null && bufloaded(buffer)
        \ && getbufvar(buffer, 'nia_attached', 0)
        \ && getbufvar(buffer, 'changedtick') == request.tick
    call s:apply(buffer, response.result)
  endif
  call s:dispatch()
endfunction

function! s:groups() abort
  return map(keys(s:roles), {_, role -> 'Nia_' . role})
endfunction

function! s:apply(buffer, result) abort
  if has('nvim')
    call nvim_buf_clear_namespace(a:buffer, s:namespace, 0, -1)
    for [line, col, length, utf16col, utf16length, role] in a:result.tokens
      call nvim_buf_add_highlight(a:buffer, s:namespace, 'Nia_' . role, line, col, col + length)
    endfor
  else
    call prop_remove({'bufnr': a:buffer, 'types': s:groups(), 'all': 1})
    let properties = {}
    for [line, col, length, utf16col, utf16length, role] in a:result.tokens
      if !has_key(properties, role)
        let properties[role] = []
      endif
      call add(properties[role], [line + 1, col + 1, line + 1, col + length + 1])
    endfor
    for [role, ranges] in items(properties)
      call prop_add_list({'bufnr': a:buffer, 'type': 'Nia_' . role}, ranges)
    endfor
  endif

  let starts = {}
  let ends = {}
  for [first, last, kind] in a:result.folds
    let starts[first] = 1
    let ends[last + 1] = get(ends, last + 1, 0) + 1
  endfor
  let levels = []
  let depth = 0
  for line in range(getbufinfo(a:buffer)[0].linecount)
    let depth -= get(ends, line, 0)
    if has_key(starts, line)
      let depth += 1
      call add(levels, '>' . depth)
    else
      call add(levels, depth)
    endif
  endfor
  if levels ==# getbufvar(a:buffer, 'nia_folds', [])
    return
  endif
  call setbufvar(a:buffer, 'nia_folds', levels)
  for window in win_findbuf(a:buffer)
    if getwinvar(window, '&foldexpr') ==# 'nia#fold(v:lnum)'
      call win_execute(window, 'let &l:foldexpr = &l:foldexpr')
    endif
  endfor
endfunction
