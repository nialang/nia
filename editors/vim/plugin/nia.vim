if exists('g:loaded_nia_editor')
  finish
endif
let g:loaded_nia_editor = 1

augroup nia_editor
  autocmd!
  autocmd BufRead,BufNewFile *.nia setfiletype nia
  autocmd FileType nia call nia#attach()
  autocmd TextChanged,TextChangedI,TextChangedP * call nia#schedule()
  autocmd BufWipeout * call nia#forget(str2nr(expand('<abuf>')))
  autocmd VimEnter * call nia#icons()
augroup END
