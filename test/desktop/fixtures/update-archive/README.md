# Desktop update archive fixtures

`valid-root.tar.gz.base64` is a small, disposable gzip-compressed POSIX tar
fixture containing only the required `Cuetrio.app/` directory.
Rust unit tests build richer disposable archives in temporary directories so
they can exercise files, symlinks, modes, and failure cleanup without storing a
real application bundle in the repository.
