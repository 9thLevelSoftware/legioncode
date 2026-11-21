//! Database handle abstraction accepted by `project_store` helpers.

use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::sqlite::SqliteStore;

use super::{ProjectStoreError, schema};

mod sealed {
    pub trait Sealed {}
}

/// Database handle accepted by project store helpers (`&Path`, `&PathBuf`, `PathBuf`,
/// `&Connection`, `&mut Connection`, `&SqliteStore`, or `&mut SqliteStore`).
pub trait ProjectDatabase: sealed::Sealed {
    /// Opens or borrows a connection suitable for reading project rows without creating files on disk.
    fn read_projects_with<T>(
        self,
        f: impl FnOnce(Option<&Connection>) -> Result<T, ProjectStoreError>,
    ) -> Result<T, ProjectStoreError>;

    /// Opens or borrows a writable connection with `projection_projects` bootstrapped.
    fn write_projects_with<T>(
        self,
        f: impl FnOnce(&Connection) -> Result<T, ProjectStoreError>,
    ) -> Result<T, ProjectStoreError>;
}

impl sealed::Sealed for &Path {}
impl ProjectDatabase for &Path {
    fn read_projects_with<T>(
        self,
        f: impl FnOnce(Option<&Connection>) -> Result<T, ProjectStoreError>,
    ) -> Result<T, ProjectStoreError> {
        let Some(conn) = schema::open_read_only(self)? else {
            return f(None);
        };
        let out = f(Some(&conn));
        if let Err((_, source)) = conn.close()
            && out.is_ok()
        {
            return Err(ProjectStoreError::Sqlite {
                operation: "close_read_only",
                source,
            });
        }
        out
    }

    fn write_projects_with<T>(
        self,
        f: impl FnOnce(&Connection) -> Result<T, ProjectStoreError>,
    ) -> Result<T, ProjectStoreError> {
        let store = schema::open_read_write(self)?;
        schema::ensure_project_store_schema(store.connection())?;
        let out = f(store.connection())?;
        store
            .close()
            .map_err(|source| ProjectStoreError::OpenDatabase {
                path: self.to_path_buf(),
                source: Box::new(source),
            })?;
        Ok(out)
    }
}

impl sealed::Sealed for &PathBuf {}
impl ProjectDatabase for &PathBuf {
    fn read_projects_with<T>(
        self,
        f: impl FnOnce(Option<&Connection>) -> Result<T, ProjectStoreError>,
    ) -> Result<T, ProjectStoreError> {
        self.as_path().read_projects_with(f)
    }

    fn write_projects_with<T>(
        self,
        f: impl FnOnce(&Connection) -> Result<T, ProjectStoreError>,
    ) -> Result<T, ProjectStoreError> {
        self.as_path().write_projects_with(f)
    }
}

impl sealed::Sealed for PathBuf {}
impl ProjectDatabase for PathBuf {
    fn read_projects_with<T>(
        self,
        f: impl FnOnce(Option<&Connection>) -> Result<T, ProjectStoreError>,
    ) -> Result<T, ProjectStoreError> {
        self.as_path().read_projects_with(f)
    }

    fn write_projects_with<T>(
        self,
        f: impl FnOnce(&Connection) -> Result<T, ProjectStoreError>,
    ) -> Result<T, ProjectStoreError> {
        self.as_path().write_projects_with(f)
    }
}

impl sealed::Sealed for &Connection {}
impl ProjectDatabase for &Connection {
    fn read_projects_with<T>(
        self,
        f: impl FnOnce(Option<&Connection>) -> Result<T, ProjectStoreError>,
    ) -> Result<T, ProjectStoreError> {
        f(Some(self))
    }

    fn write_projects_with<T>(
        self,
        f: impl FnOnce(&Connection) -> Result<T, ProjectStoreError>,
    ) -> Result<T, ProjectStoreError> {
        schema::ensure_project_store_schema(self)?;
        f(self)
    }
}

impl sealed::Sealed for &mut Connection {}
impl ProjectDatabase for &mut Connection {
    fn read_projects_with<T>(
        self,
        f: impl FnOnce(Option<&Connection>) -> Result<T, ProjectStoreError>,
    ) -> Result<T, ProjectStoreError> {
        (&*self).read_projects_with(f)
    }

    fn write_projects_with<T>(
        self,
        f: impl FnOnce(&Connection) -> Result<T, ProjectStoreError>,
    ) -> Result<T, ProjectStoreError> {
        (&*self).write_projects_with(f)
    }
}

impl sealed::Sealed for &SqliteStore {}
impl ProjectDatabase for &SqliteStore {
    fn read_projects_with<T>(
        self,
        f: impl FnOnce(Option<&Connection>) -> Result<T, ProjectStoreError>,
    ) -> Result<T, ProjectStoreError> {
        self.connection().read_projects_with(f)
    }

    fn write_projects_with<T>(
        self,
        f: impl FnOnce(&Connection) -> Result<T, ProjectStoreError>,
    ) -> Result<T, ProjectStoreError> {
        self.connection().write_projects_with(f)
    }
}

impl sealed::Sealed for &mut SqliteStore {}
impl ProjectDatabase for &mut SqliteStore {
    fn read_projects_with<T>(
        self,
        f: impl FnOnce(Option<&Connection>) -> Result<T, ProjectStoreError>,
    ) -> Result<T, ProjectStoreError> {
        (&*self).read_projects_with(f)
    }

    fn write_projects_with<T>(
        self,
        f: impl FnOnce(&Connection) -> Result<T, ProjectStoreError>,
    ) -> Result<T, ProjectStoreError> {
        (&*self).write_projects_with(f)
    }
}
