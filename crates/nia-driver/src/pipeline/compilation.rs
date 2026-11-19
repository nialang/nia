// SPDX-License-Identifier: GPL-3.0-or-later
//! Source, loader, and compiler-session workflow for the driver.

use super::*;

impl Driver {
    /// Installs or replaces an in-memory source for subsequent requests.
    pub fn set_source(
        &self,
        path: impl Into<String>,
        text: impl Into<std::sync::Arc<str>>,
    ) -> Result<(), DriverError> {
        self.set_source_path(SourcePath::new(path.into()), text)
    }

    /// Installs or replaces an in-memory source with an explicit logical identity.
    pub fn set_source_path(
        &self,
        path: SourcePath,
        text: impl Into<std::sync::Arc<str>>,
    ) -> Result<(), DriverError> {
        let loader = self.loader.lock().map_err(|_| {
            DriverError::InternalDiagnostic(Diagnostic::from(nia_ice::Ice::new(
                "driver loader state lock is poisoned",
            )))
        })?;
        if let Some(loader) = &*loader {
            loader
                .database
                .set_source_path(path, text)
                .map(drop)
                .map_err(|error| DriverError::InternalDiagnostic(query_error_diagnostic(error)))
        } else {
            drop(loader);
            self.sources
                .set_source(path, text)
                .map(drop)
                .map_err(|error| {
                    DriverError::InternalDiagnostic(Diagnostic::from(nia_ice::Ice::new(
                        error.to_string(),
                    )))
                })
        }
    }

    /// Checks every module reachable from the request's module map.
    pub fn check_all_modules(&self, request: CheckRequest) -> DriverOutput<CheckedProgram> {
        let program = match self.check_all_modules_inner(request) {
            Ok(program) => program,
            Err(error) => {
                return DriverOutput::from_error(DriverError::InternalDiagnostic(
                    query_error_diagnostic(error),
                ));
            }
        };
        if has_error_diagnostics(&program.diagnostics) {
            return DriverOutput::from_check_diagnostics(program);
        }
        DriverOutput::success(program)
    }

    /// Returns the final source manifest for an entry request.
    pub fn source_input_manifest(
        &self,
        request: &CheckRequest,
    ) -> DriverOutput<SourceInputManifest> {
        let loader = match self.loader_database(request, false) {
            Ok(loader) => loader,
            Err(error) => {
                return DriverOutput::from_error(DriverError::InternalDiagnostic(
                    query_error_diagnostic(error),
                ));
            }
        };
        if let Err(error) = loader.load_program() {
            return DriverOutput::from_error(DriverError::InternalDiagnostic(
                query_error_diagnostic(error),
            ));
        }
        match loader.source_input_manifest() {
            Ok(manifest) => DriverOutput::success(manifest),
            Err(error) => DriverOutput::from_error(DriverError::InternalDiagnostic(
                query_error_diagnostic(error),
            )),
        }
    }

    fn check_all_modules_inner(
        &self,
        request: CheckRequest,
    ) -> nia_query::QueryResult<CheckedProgram> {
        self.compile_with(request, RootScope::Tree, CompilerDatabase::check_program)
    }

    #[cfg(test)]
    pub(crate) fn analyze_all_modules(
        &self,
        request: CheckRequest,
    ) -> nia_compiler_query::CheckedProgramAnalysis {
        self.compile_with(request, RootScope::Tree, CompilerDatabase::analyze_program)
            .expect("test compiler analysis")
    }

    /// Checks every runtime body of the request's module tree, and the
    /// dependencies as far as the tree reaches into them.
    pub fn check(&self, request: CheckRequest) -> DriverOutput<CheckedProgram> {
        let program = match self.check_inner(request) {
            Ok(program) => program,
            Err(error) => {
                return DriverOutput::from_error(DriverError::InternalDiagnostic(
                    query_error_diagnostic(error),
                ));
            }
        };
        if has_error_diagnostics(&program.diagnostics) {
            return DriverOutput::from_check_diagnostics(program);
        }
        DriverOutput::success(program)
    }

    /// Runs editor analysis against the same module scope as `check`.
    ///
    /// The consumer runs against the checked revision, including when the
    /// program contains user errors, so it can resolve source origins.
    pub fn analyze_for_editor<T>(
        &self,
        request: CheckRequest,
        cancel: Option<&nia_query::CancellationToken>,
        consume: impl FnOnce(
            &CompilerDatabase,
            nia_compiler_query::CheckedProgramAnalysis,
            &LoaderDatabase,
        ) -> nia_query::QueryResult<T>,
    ) -> nia_query::QueryResult<T> {
        let checkpoint = || cancel.map_or(Ok(()), nia_query::CancellationToken::check);
        checkpoint()?;
        let timings = request.timings;
        let (database, loader) = nia_timing::time_stage(
            timings,
            nia_timing::TimingLevel::Summary,
            "editor.load_graph",
            || self.compilation_databases(&request, RootScope::Tree, true),
        )?;
        let analysis = nia_timing::time_stage(
            timings,
            nia_timing::TimingLevel::Summary,
            "editor.semantic_queries",
            || database.analyze_entry_program_with_cancellation(cancel),
        )?;
        checkpoint()?;
        nia_timing::time_stage(
            timings,
            nia_timing::TimingLevel::Summary,
            "editor.consume_analysis",
            || consume(&database, analysis, &loader),
        )
    }

    /// Checks the requested roots and returns the exact source manifest alongside.
    /// Cache discovery must use the same roots as the action consuming it.
    pub fn check_with_source_manifest(
        &self,
        request: CheckRequest,
        root_scope: RootScope,
    ) -> DriverOutput<CheckedProgramWithSourceManifest> {
        let checked = match self.check_with_source_manifest_inner(request, root_scope) {
            Ok(checked) => checked,
            Err(error) => {
                return DriverOutput::from_error(DriverError::InternalDiagnostic(
                    query_error_diagnostic(error),
                ));
            }
        };
        if has_error_diagnostics(&checked.program.diagnostics) {
            return DriverOutput::from_check_diagnostics(checked.program);
        }
        DriverOutput::success(checked)
    }

    fn check_inner(&self, request: CheckRequest) -> nia_query::QueryResult<CheckedProgram> {
        self.compile_with(
            request,
            RootScope::Tree,
            CompilerDatabase::entry_check_program,
        )
    }

    fn check_with_source_manifest_inner(
        &self,
        request: CheckRequest,
        root_scope: RootScope,
    ) -> nia_query::QueryResult<CheckedProgramWithSourceManifest> {
        let (program, source_manifest) = self.compile_with_source_manifest(
            request,
            root_scope,
            CompilerDatabase::entry_check_program,
        )?;
        Ok(CheckedProgramWithSourceManifest {
            program,
            source_manifest,
        })
    }

    #[cfg(test)]
    pub(crate) fn analyze_entry_program(
        &self,
        request: CheckRequest,
    ) -> nia_compiler_query::CheckedProgramAnalysis {
        self.compile_with(
            request,
            RootScope::Tree,
            CompilerDatabase::analyze_entry_program,
        )
        .expect("test entry compiler analysis")
    }

    /// Checks and lowers the request into a code-generation program.
    pub fn codegen(&self, request: CheckRequest) -> DriverOutput<CodegenProgram> {
        let program = match self.codegen_inner(request) {
            Ok(program) => program,
            Err(error) => {
                return DriverOutput::from_error(DriverError::InternalDiagnostic(
                    query_error_diagnostic(error),
                ));
            }
        };
        if has_error_diagnostics(&program.diagnostics) {
            return DriverOutput::from_codegen_diagnostics(program);
        }
        DriverOutput::success(program)
    }

    fn codegen_inner(&self, request: CheckRequest) -> nia_query::QueryResult<CodegenProgram> {
        self.compile_with(request, RootScope::Entry, CompilerDatabase::codegen_program)
    }

    fn compile_with<T>(
        &self,
        request: CheckRequest,
        root_scope: RootScope,
        compile: impl Fn(&CompilerDatabase) -> nia_query::QueryResult<T>,
    ) -> nia_query::QueryResult<T>
    where
        T: ProviderDemandOutput,
    {
        let timings = request.timings;
        let database = self.compiler_database(&request, root_scope)?;
        let output = compile(&database)?;
        let loader_trace = self.loader_query_trace()?;
        emit_compilation_counters(
            timings,
            &database,
            &loader_trace,
            &output,
            database.provider_demand_rounds(),
            self.sources.source_table_stats(),
        )?;
        Ok(output)
    }

    fn compile_with_source_manifest<T>(
        &self,
        request: CheckRequest,
        root_scope: RootScope,
        compile: impl Fn(&CompilerDatabase) -> nia_query::QueryResult<T>,
    ) -> nia_query::QueryResult<(T, SourceInputManifest)>
    where
        T: ProviderDemandOutput,
    {
        let timings = request.timings;
        let (database, loader) = self.compilation_databases(&request, root_scope, false)?;
        let output = compile(&database)?;
        let source_manifest = loader.source_input_manifest()?;
        let loader_trace = self.loader_query_trace()?;
        emit_compilation_counters(
            timings,
            &database,
            &loader_trace,
            &output,
            database.provider_demand_rounds(),
            self.sources.source_table_stats(),
        )?;
        Ok((output, source_manifest))
    }

    pub(super) fn compiler_database(
        &self,
        request: &CheckRequest,
        root_scope: RootScope,
    ) -> nia_query::QueryResult<CompilerDatabase> {
        self.compilation_databases(request, root_scope, false)
            .map(|(compiler, _)| compiler)
    }

    pub(super) fn compilation_databases(
        &self,
        request: &CheckRequest,
        root_scope: RootScope,
        recover_declarations: bool,
    ) -> nia_query::QueryResult<(CompilerDatabase, LoaderDatabase)> {
        let loader = self.loader_database(request, recover_declarations)?;
        loader.load_program()?;
        let query_session = loader.query_session();
        let frontend_cache = self
            .config
            .artifact_cache_dir
            .clone()
            .filter(|_| !recover_declarations);
        let mut compiler_guard = self.compiler.lock().map_err(|_| {
            nia_query::QueryError::internal("driver compiler state lock is poisoned")
        })?;
        let database = if let Some(compiler) = &*compiler_guard
            && compiler.database.query_session().ptr_eq(&query_session)
        {
            compiler.database.update(
                CompileRequest::new(loader.clone())
                    .with_optimization(request.optimization)
                    .with_timings(request.timings)
                    .with_root_scope(root_scope)
                    .with_current_package(request.current_package.clone())
                    .with_frontend_cache_dir(frontend_cache)
                    .with_frontend_cache_verification(self.config.verify_frontend_cache),
            )?;
            compiler.database.clone()
        } else {
            let database = CompilerDatabase::new(
                CompileRequest::new(loader.clone())
                    .with_optimization(request.optimization)
                    .with_timings(request.timings)
                    .with_root_scope(root_scope)
                    .with_current_package(request.current_package.clone())
                    .with_frontend_cache_dir(frontend_cache)
                    .with_frontend_cache_verification(self.config.verify_frontend_cache),
            )?;
            *compiler_guard = Some(SessionCompiler {
                database: database.clone(),
            });
            database
        };
        drop(compiler_guard);
        Ok((database, loader))
    }
}
