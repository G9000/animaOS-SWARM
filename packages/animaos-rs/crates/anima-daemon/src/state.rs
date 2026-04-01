use anima_memory::MemoryManager;
use anima_swarm::SwarmCoordinator;

pub(crate) struct DaemonState {
    pub(crate) memory: MemoryManager,
    pub(crate) _swarm: SwarmCoordinator,
}

impl DaemonState {
    pub(crate) fn new() -> Self {
        Self {
            memory: MemoryManager::new(),
            _swarm: SwarmCoordinator::new(),
        }
    }
}
