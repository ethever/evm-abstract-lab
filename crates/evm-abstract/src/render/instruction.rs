//! 所有指令列表共用的块标题、PC 列与三地址正文。
//!
//! 调用方分别组织状态/帧元数据、phi、效果和转移；这里不合并这些语义身份。
//! 布局只用于真实字节码块，原生调用和合成继续点不得制造 PC。

use crate::ssa::Instruction;
use revm_bytecode::opcode::OpCode;
use std::fmt::Write;

/// PC 数字列由实际块编号宽度决定，供反汇编和全部 SSA 视图复用。
pub(super) struct InstructionLayout {
    pc_column: usize,
}

impl InstructionLayout {
    /// 块标题顶格；至少四位十六进制，不截断更大的 PC。
    pub(super) fn block(output: &mut String, block_id: usize, pc: usize) -> Self {
        let prefix = format!("B{block_id} @ 0x");
        writeln!(output, "{prefix}{pc:04x}:").unwrap();
        Self {
            pc_column: prefix.len(),
        }
    }

    /// 栈等列表内容沿用 PC 数字列，不另行硬编码缩进。
    pub(super) fn indent(&self, output: &mut String) {
        write!(output, "{:width$}", "", width = self.pc_column).unwrap();
    }

    /// 指令与指令效果的地址不带 0x，并与块标题的 PC 数字首位对齐。
    pub(super) fn write_pc(&self, output: &mut String, pc: usize) {
        self.indent(output);
        write!(output, "{pc:04x}: ").unwrap();
    }
}

/// 单程序和 world 教学 SSA 共用定义/使用顺序；注释、PC 和换行由视图组织。
pub(super) fn write_ssa_body(output: &mut String, instruction: &Instruction) {
    if !instruction.results.is_empty() {
        let results = instruction
            .results
            .iter()
            .map(|value| format!("%{value}"))
            .collect::<Vec<_>>()
            .join(", ");
        write!(output, "{results} = ").unwrap();
    }
    write!(output, "{}", OpCode::name_by_op(instruction.opcode)).unwrap();
    if let Some(immediate) = instruction.immediate {
        write!(output, " 0x{immediate:x}").unwrap();
    }
    for value in &instruction.operands {
        write!(output, " %{value}").unwrap();
    }
}
