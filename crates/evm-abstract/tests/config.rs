//! 配置准入的边界与实际执行一致，不能在引擎中重新猜测或采用默认域容量。

use evm_abstract::{
    U256,
    analysis::{self, Config, ConfigError},
    bytecode::Program,
};

#[test]
fn validation_reports_typed_errors_without_entering_the_engine() {
    for capacity in [0, 65, usize::MAX] {
        assert!(matches!(
            Config {
                max_constants: capacity,
                ..Config::default()
            }
            .validate(),
            Err(ConfigError::Constants)
        ));
    }
    for config in [
        Config {
            max_states: 0,
            ..Config::default()
        },
        Config {
            max_transfers: 0,
            ..Config::default()
        },
    ] {
        assert!(matches!(config.validate(), Err(ConfigError::Budget)));
    }
}

#[test]
fn validated_boundary_values_keep_their_original_options() {
    for capacity in [1, 64] {
        for depth in [0, 3, 8, 10, usize::MAX] {
            let validated = Config {
                max_constants: capacity,
                context_depth: depth,
                max_states: 1,
                max_transfers: 1,
                ..Config::default()
            }
            .validate()
            .unwrap();
            assert_eq!(validated.config().max_constants, capacity);
            assert_eq!(validated.config().context_depth, depth);
            assert_eq!(validated.config().max_states, 1);
            assert_eq!(validated.config().max_transfers, 1);
        }
    }
}

#[test]
fn default_context_depth_is_eight() {
    assert_eq!(Config::default().context_depth, 8);
}

#[test]
fn admitted_domain_capacity_controls_the_actual_join() {
    let diamond = "600035600b576002600e565b60015b600a0100";
    for capacity in [1, 2] {
        let result = analysis::analyze(
            Program::from_hex(diamond).unwrap(),
            Config {
                max_constants: capacity,
                // 本测试检查域容量对汇合的影响，主动关闭上下文区分。
                context_depth: 0,
                ..Config::default()
            },
        )
        .unwrap();
        let merge = result
            .states()
            .iter()
            .find(|state| result.program().blocks()[state.key.basic_block_index].start_pc == 14)
            .unwrap();
        assert_eq!(result.config().max_constants, capacity);
        if capacity == 1 {
            assert!(merge.entry_stack[0].constants().is_none());
        } else {
            let values = merge.entry_stack[0].constants().unwrap();
            assert_eq!(values.len(), 2);
            assert!(values.contains(&U256::from(1)) && values.contains(&U256::from(2)));
        }
    }
}
