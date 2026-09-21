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
        let path = path.into();
        let loader = self.loader.lock().map_err(|_| {
            DriverError::InternalDiagnostic(Diagnostic::from(nia_ice::Ice::new(
                "driver loader state lock is poisoned",
            )))
        })?;
        if let Some(loader) = &*loader {
            loader
                .database
                .set_source(path, text)
                .map(drop)
                .map_err(|error| DriverError::InternalDiagnostic(query_error_diagnostic(error)))
        } else {
            drop(loader);
            self.sources
                .set_source(SourcePath::new(path), text)
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
        let loader = match self.loader_database(request) {
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
        self.compile_with(request, CompilerDatabase::check_program)
    }

    #[cfg(test)]
    pub(crate) fn analyze_all_modules(
        &self,
        request: CheckRequest,
    ) -> nia_compiler_query::CheckedProgramAnalysis {
        self.compile_with(request, CompilerDatabase::analyze_program)
            .expect("test compiler analysis")
    }

    /// Checks the request entry and its reachable semantic program.
    pub fn check_entry(&self, request: CheckRequest) -> DriverOutput<CheckedProgram> {
        let program = match self.check_entry_inner(request) {
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

    /// Checks an entry and returns its exact source manifest alongside it.
    pub fn check_entry_with_source_manifest(
        &self,
        request: CheckRequest,
    ) -> DriverOutput<CheckedProgramWithSourceManifest> {
        let checked = match self.check_entry_with_source_manifest_inner(request) {
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

    fn check_entry_inner(&self, request: CheckRequest) -> nia_query::QueryResult<CheckedProgram> {
        self.compile_with(request, CompilerDatabase::entry_check_program)
    }

    fn check_entry_with_source_manifest_inner(
        &self,
        request: CheckRequest,
    ) -> nia_query::QueryResult<CheckedProgramWithSourceManifest> {
        let (program, source_manifest) =
            self.compile_with_source_manifest(request, CompilerDatabase::entry_check_program)?;
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
        self.compile_with(request, CompilerDatabase::analyze_entry_program)
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
        self.compile_with(request, CompilerDatabase::codegen_program)
    }

    fn compile_with<T>(
        &self,
        request: CheckRequest,
        compile: impl Fn(&CompilerDatabase) -> nia_query::QueryResult<T>,
    ) -> nia_query::QueryResult<T>
    where
        T: ProviderDemandOutput,
    {
        let timings = request.timings;
        let database = self.compiler_database(&request)?;
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
        compile: impl Fn(&CompilerDatabase) -> nia_query::QueryResult<T>,
    ) -> nia_query::QueryResult<(T, SourceInputManifest)>
    where
        T: ProviderDemandOutput,
    {
        let timings = request.timings;
        let (database, loader) = self.compilation_databases(&request)?;
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
    ) -> nia_query::QueryResult<CompilerDatabase> {
        self.compilation_databases(request)
            .map(|(compiler, _)| compiler)
    }

    fn compilation_databases(
        &self,
        request: &CheckRequest,
    ) -> nia_query::QueryResult<(CompilerDatabase, LoaderDatabase)> {
        self.compilation_databases_with_codegen_scope(request, CodegenScope::Entry)
    }

    pub(super) fn compilation_databases_with_codegen_scope(
        &self,
        request: &CheckRequest,
        codegen_scope: CodegenScope,
    ) -> nia_query::QueryResult<(CompilerDatabase, LoaderDatabase)> {
        let loader = self.loader_database(request)?;
        loader.load_program()?;
        let query_session = loader.query_session();
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
                    .with_codegen_scope(codegen_scope)
                    .with_current_package(request.current_package.clone())
                    .with_frontend_cache_dir(self.config.artifact_cache_dir.clone())
                    .with_frontend_cache_verification(self.config.verify_frontend_cache),
            )?;
            compiler.database.clone()
        } else {
            let database = CompilerDatabase::new(
                CompileRequest::new(loader.clone())
                    .with_optimization(request.optimization)
                    .with_timings(request.timings)
                    .with_codegen_scope(codegen_scope)
                    .with_current_package(request.current_package.clone())
                    .with_frontend_cache_dir(self.config.artifact_cache_dir.clone())
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
