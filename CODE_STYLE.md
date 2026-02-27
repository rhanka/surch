# Code Style Guide - Surch

## Rust Conventions

### Formatting
- Use `cargo fmt` for formatting
- 4 spaces for indentation
- Maximum line length: 100 characters

### Naming
- **Snake_case** for functions, variables, and modules
- **PascalCase** for types and traits
- **SCREAMING_SNAKE_CASE** for constants
- Prefixes: `get_`, `set_`, `is_`, `has_` for getters/setters

### Imports
- Group imports: standard library, external crates, local modules
- Use absolute imports for project modules

### Error Handling
- Use `thiserror` for error types
- Return `Result<T, Error>` for fallible operations
- Prefer descriptive error messages

### Documentation
- Document public APIs with doc comments (`///`)
- Include examples in doc comments where helpful

### Testing
- Unit tests in `#[cfg(test)]` modules
- Integration tests in `tests/` directory
- Minimum 80% coverage for core modules

### Security
- Validate all inputs at API boundaries
- No unsafe code unless documented
- Use secure dependencies
