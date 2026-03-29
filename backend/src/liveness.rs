use machina_core::op::LifeData;
use machina_core::temp::TempKind;
use machina_core::{Context, OpFlags, Opcode, OPCODE_DEFS};

/// Perform backward liveness analysis over the IR ops in `ctx`.
///
/// Sets `LifeData` on each op indicating which arguments are
/// dead after the op and which need to be synced to memory.
pub fn liveness_analysis(ctx: &mut Context) {
    let nb_temps = ctx.nb_temps() as usize;
    let nb_globals = ctx.nb_globals() as usize;

    // temp_state[i] = true means temp i is live
    let mut temp_state = vec![false; nb_temps];

    // At end of TB, all globals are live
    for s in temp_state.iter_mut().take(nb_globals) {
        *s = true;
    }

    let num_ops = ctx.num_ops();

    // Walk ops in reverse
    for oi in (0..num_ops).rev() {
        let op = ctx.ops()[oi].clone();
        let def = &OPCODE_DEFS[op.opc as usize];
        let flags = def.flags;

        // At BB_END, mark all globals live
        if flags.contains(OpFlags::BB_END) {
            for s in temp_state.iter_mut().take(nb_globals) {
                *s = true;
            }
        }

        // Skip ops that don't produce host code and have
        // no liveness impact beyond BB_END handling above.
        if op.opc == Opcode::Nop || op.opc == Opcode::InsnStart {
            continue;
        }

        let mut life = LifeData(0);
        let nb_oargs = def.nb_oargs as usize;
        let nb_iargs = def.nb_iargs as usize;

        // Process output args
        for i in 0..nb_oargs {
            let tidx = op.args[i].0 as usize;
            if tidx < nb_temps && !temp_state[tidx] {
                life.set_dead(i as u32);
            }
            if tidx < nb_temps {
                temp_state[tidx] = false;
            }
        }

        // Process input args
        for i in 0..nb_iargs {
            let arg_pos = nb_oargs + i;
            let tidx = op.args[arg_pos].0 as usize;
            if tidx >= nb_temps {
                continue;
            }
            if !temp_state[tidx] {
                // Last use — mark dead
                life.set_dead(arg_pos as u32);
                // If global, needs sync before death
                let kind = ctx.temp(machina_core::TempIdx(tidx as u32)).kind;
                if kind == TempKind::Global {
                    life.set_sync(arg_pos as u32);
                }
            }
            temp_state[tidx] = true;
        }

        // Store computed life data back
        let op_mut = ctx.op_mut(op.idx);
        op_mut.life = life;
    }
}
