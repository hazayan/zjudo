use crate::error::{BootError, Result};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

/// Module dependency information
#[derive(Debug, Clone)]
pub struct ModuleDependency {
    pub name: String,
    pub path: String,
    pub dependencies: Vec<String>,
    pub provided_symbols: Vec<String>,
    pub required_symbols: Vec<String>,
}

/// Module dependency graph
pub struct DependencyGraph {
    modules: HashMap<String, ModuleDependency>,
    adjacency: HashMap<String, Vec<String>>,
    reverse_adjacency: HashMap<String, Vec<String>>,
}

impl DependencyGraph {
    pub fn new() -> Self {
        Self {
            modules: HashMap::new(),
            adjacency: HashMap::new(),
            reverse_adjacency: HashMap::new(),
        }
    }

    /// Add a module to the graph
    pub fn add_module(&mut self, module: ModuleDependency) {
        let name = module.name.clone();
        self.modules.insert(name.clone(), module);
        self.adjacency.insert(name.clone(), Vec::new());
        self.reverse_adjacency.insert(name.clone(), Vec::new());
    }

    /// Add a dependency edge
    pub fn add_dependency(&mut self, from: &str, to: &str) -> Result<()> {
        if !self.modules.contains_key(from) {
            return Err(BootError::Module(format!("Module {} not found", from)));
        }
        if !self.modules.contains_key(to) {
            return Err(BootError::Module(format!("Module {} not found", to)));
        }

        self.adjacency
            .entry(from.to_string())
            .or_insert_with(Vec::new)
            .push(to.to_string());

        self.reverse_adjacency
            .entry(to.to_string())
            .or_insert_with(Vec::new)
            .push(from.to_string());

        Ok(())
    }

    /// Get topological order of modules (dependency order)
    pub fn topological_order(&self) -> Result<Vec<String>> {
        let mut in_degree = HashMap::new();
        let mut queue = VecDeque::new();
        let mut result = Vec::new();

        // Initialize in-degree for each module
        for module_name in self.modules.keys() {
            let degree = self
                .reverse_adjacency
                .get(module_name)
                .map_or(0, |deps| deps.len());
            in_degree.insert(module_name.clone(), degree);

            if degree == 0 {
                queue.push_back(module_name.clone());
            }
        }

        // Kahn's algorithm for topological sorting
        while let Some(module_name) = queue.pop_front() {
            result.push(module_name.clone());

            if let Some(dependents) = self.adjacency.get(&module_name) {
                for dependent in dependents {
                    let degree = in_degree
                        .get_mut(dependent)
                        .ok_or_else(|| BootError::Module("Module not found in in-degree".to_string()))?;
                    
                    *degree -= 1;
                    if *degree == 0 {
                        queue.push_back(dependent.clone());
                    }
                }
            }
        }

        // Check for cycles
        if result.len() != self.modules.len() {
            return Err(BootError::Module(
                "Cyclic dependency detected in module graph".to_string(),
            ));
        }

        Ok(result)
    }

    /// Get modules in load order (dependencies first)
    pub fn load_order(&self) -> Result<Vec<String>> {
        self.topological_order()
    }

    /// Get modules in unload order (dependents first)
    pub fn unload_order(&self) -> Result<Vec<String>> {
        let mut order = self.topological_order()?;
        order.reverse();
        Ok(order)
    }

    /// Find all dependencies for a module (transitive closure)
    pub fn find_all_dependencies(&self, module_name: &str) -> Result<HashSet<String>> {
        let mut visited = HashSet::new();
        let mut stack = vec![module_name.to_string()];

        while let Some(current) = stack.pop() {
            if visited.contains(&current) {
                continue;
            }

            visited.insert(current.clone());

            // Follow reverse edges to find what this module depends on
            if let Some(dependencies) = self.reverse_adjacency.get(&current) {
                for dep in dependencies {
                    if !visited.contains(dep) {
                        stack.push(dep.clone());
                    }
                }
            }
        }

        // Remove the module itself from its dependencies
        visited.remove(module_name);

        Ok(visited)
    }

    /// Find all modules that depend on a given module
    pub fn find_all_dependents(&self, module_name: &str) -> Result<HashSet<String>> {
        let mut visited = HashSet::new();
        let mut stack = vec![module_name.to_string()];

        while let Some(current) = stack.pop() {
            if visited.contains(&current) {
                continue;
            }

            visited.insert(current.clone());

            // Follow forward edges to find what depends on this module
            if let Some(dependents) = self.adjacency.get(&current) {
                for dep in dependents {
                    if !visited.contains(dep) {
                        stack.push(dep.clone());
                    }
                }
            }
        }

        // Remove the module itself from its dependents
        visited.remove(module_name);

        Ok(visited)
    }

    /// Check if adding a dependency would create a cycle
    pub fn would_create_cycle(&self, from: &str, to: &str) -> bool {
        // If 'to' depends on 'from' (directly or indirectly), adding 'from' -> 'to' would create a cycle
        if let Ok(deps) = self.find_all_dependencies(to) {
            deps.contains(from)
        } else {
            false
        }
    }

    /// Resolve symbol dependencies between modules
    pub fn resolve_symbols(&self) -> Result<HashMap<String, Vec<String>>> {
        let mut symbol_map = HashMap::new();
        let mut unresolved_symbols = HashMap::new();

        // First pass: collect all provided symbols
        for (module_name, module) in &self.modules {
            for symbol in &module.provided_symbols {
                symbol_map
                    .entry(symbol.clone())
                    .or_insert_with(Vec::new)
                    .push(module_name.clone());
            }
        }

        // Second pass: check required symbols
        for (module_name, module) in &self.modules {
            for symbol in &module.required_symbols {
                if !symbol_map.contains_key(symbol) {
                    unresolved_symbols
                        .entry(module_name.clone())
                        .or_insert_with(Vec::new)
                        .push(symbol.clone());
                }
            }
        }

        if !unresolved_symbols.is_empty() {
            let mut error_msg = String::from("Unresolved symbols:\n");
            for (module, symbols) in unresolved_symbols {
                error_msg.push_str(&format!("  {}: {}\n", module, symbols.join(", ")));
            }
            return Err(BootError::Module(error_msg));
        }

        Ok(symbol_map)
    }

    /// Get module by name
    pub fn get_module(&self, name: &str) -> Option<&ModuleDependency> {
        self.modules.get(name)
    }

    /// Get all modules
    pub fn get_modules(&self) -> &HashMap<String, ModuleDependency> {
        &self.modules
    }

    /// Check if module exists
    pub fn has_module(&self, name: &str) -> bool {
        self.modules.contains_key(name)
    }

    /// Get number of modules
    pub fn len(&self) -> usize {
        self.modules.len()
    }

    /// Check if graph is empty
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }
}

/// Parse module dependencies from FreeBSD kernel module
pub fn parse_module_dependencies(
    module_path: &Path,
    module_name: &str,
) -> Result<ModuleDependency> {
    // This is a simplified implementation
    // In practice, we would need to parse the ELF file to extract:
    // - Module dependencies from .modinfo section
    // - Provided symbols from .symtab
    // - Required symbols from .dynsym

    let mut dependencies = Vec::new();
    let mut provided_symbols = Vec::new();
    let required_symbols = Vec::new();

    // Try to read module info
    if let Ok(content) = std::fs::read(module_path) {
        // Simple heuristic: look for dependency patterns
        // In real implementation, use goblin to parse ELF
        let content_str = String::from_utf8_lossy(&content);
        
        // Look for typical FreeBSD module dependencies
        if content_str.contains("kernel") {
            dependencies.push("kernel".to_string());
        }
        
        // Add module name as a provided symbol
        provided_symbols.push(module_name.to_string());
    }

    Ok(ModuleDependency {
        name: module_name.to_string(),
        path: module_path.to_string_lossy().to_string(),
        dependencies,
        provided_symbols,
        required_symbols,
    })
}

/// Build dependency graph from a list of module paths
pub fn build_dependency_graph(module_paths: &[(&str, &Path)]) -> Result<DependencyGraph> {
    let mut graph = DependencyGraph::new();

    // First pass: add all modules
    for (module_name, module_path) in module_paths {
        let dependency = parse_module_dependencies(module_path, module_name)?;
        graph.add_module(dependency);
    }

    // Second pass: add dependencies
    // Collect dependencies first to avoid borrowing issues
    let mut dependencies_to_add = Vec::new();
    
    for (module_name, _) in module_paths {
        if let Some(module) = graph.get_module(module_name) {
            for dep in &module.dependencies {
                if graph.has_module(dep) {
                    dependencies_to_add.push((module_name.to_string(), dep.clone()));
                }
            }
        }
    }

    // Add all dependencies
    for (from, to) in dependencies_to_add {
        graph.add_dependency(&from, &to)?;
    }

    Ok(graph)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dependency_graph_creation() {
        let graph = DependencyGraph::new();
        assert!(graph.is_empty());
        assert_eq!(graph.len(), 0);
    }

    #[test]
    fn test_add_module() {
        let mut graph = DependencyGraph::new();
        
        let module = ModuleDependency {
            name: "test".to_string(),
            path: "/test.ko".to_string(),
            dependencies: vec![],
            provided_symbols: vec![],
            required_symbols: vec![],
        };
        
        graph.add_module(module);
        assert!(!graph.is_empty());
        assert_eq!(graph.len(), 1);
        assert!(graph.has_module("test"));
    }

    #[test]
    fn test_topological_order() {
        let mut graph = DependencyGraph::new();
        
        // Create modules
        let kernel = ModuleDependency {
            name: "kernel".to_string(),
            path: "".to_string(),
            dependencies: vec![],
            provided_symbols: vec![],
            required_symbols: vec![],
        };
        
        let module1 = ModuleDependency {
            name: "module1".to_string(),
            path: "".to_string(),
            dependencies: vec!["kernel".to_string()],
            provided_symbols: vec![],
            required_symbols: vec![],
        };
        
        let module2 = ModuleDependency {
            name: "module2".to_string(),
            path: "".to_string(),
            dependencies: vec!["module1".to_string()],
            provided_symbols: vec![],
            required_symbols: vec![],
        };
        
        graph.add_module(kernel);
        graph.add_module(module1);
        graph.add_module(module2);
        
        // Add dependencies: kernel must come before module1, module1 before module2
        // So edges are: kernel -> module1, module1 -> module2
        graph.add_dependency("kernel", "module1").unwrap();
        graph.add_dependency("module1", "module2").unwrap();
        
        let order = graph.topological_order().unwrap();
        
        // Check order: kernel -> module1 -> module2
        assert_eq!(order.len(), 3);
        assert_eq!(order[0], "kernel");
        assert_eq!(order[1], "module1");
        assert_eq!(order[2], "module2");
    }

    #[test]
    fn test_find_all_dependencies() {
        let mut graph = DependencyGraph::new();
        
        // Create a simple dependency chain
        let modules = vec![
            ("kernel", vec![]),
            ("module1", vec!["kernel"]),
            ("module2", vec!["module1"]),
            ("module3", vec!["module2"]),
        ];
        
        for (name, deps) in modules {
            let module = ModuleDependency {
                name: name.to_string(),
                path: "".to_string(),
                dependencies: deps.iter().map(|s| s.to_string()).collect(),
                provided_symbols: vec![],
                required_symbols: vec![],
            };
            graph.add_module(module);
        }
        
        // Add dependencies: kernel before module1, module1 before module2, module2 before module3
        graph.add_dependency("kernel", "module1").unwrap();
        graph.add_dependency("module1", "module2").unwrap();
        graph.add_dependency("module2", "module3").unwrap();
        
        let deps = graph.find_all_dependencies("module3").unwrap();
        
        assert_eq!(deps.len(), 3);
        assert!(deps.contains("kernel"));
        assert!(deps.contains("module1"));
        assert!(deps.contains("module2"));
    }

    #[test]
    fn test_cycle_detection() {
        let mut graph = DependencyGraph::new();
        
        let module1 = ModuleDependency {
            name: "module1".to_string(),
            path: "".to_string(),
            dependencies: vec!["module2".to_string()],
            provided_symbols: vec![],
            required_symbols: vec![],
        };
        
        let module2 = ModuleDependency {
            name: "module2".to_string(),
            path: "".to_string(),
            dependencies: vec!["module1".to_string()],
            provided_symbols: vec![],
            required_symbols: vec![],
        };
        
        graph.add_module(module1);
        graph.add_module(module2);
        
        graph.add_dependency("module1", "module2").unwrap();
        graph.add_dependency("module2", "module1").unwrap();
        
        let result = graph.topological_order();
        assert!(result.is_err());
        // Check for cycle detection error (could be "cyclic" or "cycle" or "Cyclic")
        let error_msg = result.unwrap_err().to_string().to_lowercase();
        assert!(error_msg.contains("cyclic") || error_msg.contains("cycle"));
    }
}
