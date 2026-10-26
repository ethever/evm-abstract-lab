use super::{
    Builder, CardKind, ControlOrigin, Destination, EdgeLevel, FlowOrigin, PointKey, ProjectionCard,
    ProjectionError, TargetEntry, TargetSet, TargetSetKind, edge,
};
use evm_abstract_protocol::{ProcessControlRelation, ProcessLocation, RegionalNode};

impl<'a> Builder<'a> {
    pub(super) fn sources(&mut self) -> Result<(), ProjectionError> {
        for node in self.members.clone() {
            self.add_point(node, None, false, false, self.program(node)?);
            if self.overview {
                continue;
            }
            let Some((region, relation)) = self.directory(node)? else {
                continue;
            };
            for &location in &region.through {
                self.add_point(node, Some(location), true, false, relation.program);
            }
            for &location in &region.boundaries {
                self.add_point(node, Some(location), false, true, relation.program);
            }
        }
        Ok(())
    }

    fn add_point(
        &mut self,
        node: &RegionalNode,
        location: Option<ProcessLocation>,
        through: bool,
        boundary: bool,
        program: Option<usize>,
    ) {
        let point = location.or_else(|| node.frame.process.as_ref().map(|frame| frame.location));
        let key = match (self.overview, point) {
            (false, Some(point)) => PointKey::Source(point),
            _ => PointKey::Receipt(node.id),
        };
        let origin = match (location, node.frame.process.as_ref()) {
            (Some(location), Some(frame)) if location == frame.location => None,
            _ => location,
        };
        let evidence = self.evidence(node, origin, through, boundary);
        let id = if let Some(&id) = self.groups.get(&key) {
            let card = &mut self.projection.cards[id];
            if let Some(previous) = card
                .origins
                .iter_mut()
                .find(|row| row.target == evidence.target)
            {
                previous.through |= through;
                previous.boundary |= boundary;
            } else {
                card.origins.push(evidence);
            }
            id
        } else {
            let id = self.projection.cards.len();
            self.projection.cards.push(ProjectionCard {
                kind: if self.overview {
                    CardKind::Receipt
                } else {
                    point.map_or(CardKind::Receipt, |location| CardKind::Source {
                        program,
                        location,
                    })
                },
                origins: vec![evidence],
                target_set: None,
            });
            self.groups.insert(key, id);
            id
        };
        self.points.insert(
            ControlOrigin {
                node: node.id,
                location,
            },
            id,
        );
        if let Some(point) = point {
            self.points.insert(
                ControlOrigin {
                    node: node.id,
                    location: Some(point),
                },
                id,
            );
        }
    }

    fn program(&self, node: &'a RegionalNode) -> Result<Option<usize>, ProjectionError> {
        if let Some((_, relation)) = self.directory(node)? {
            return Ok(relation.program);
        }
        Ok(node
            .frame
            .process
            .as_ref()
            .and_then(|frame| self.report.process_program(&frame.source))
            .map(|p| p.id))
    }

    fn directory(
        &self,
        node: &'a RegionalNode,
    ) -> Result<
        Option<(
            &'a evm_abstract_protocol::ProcessControlRegion,
            &'a ProcessControlRelation,
        )>,
        ProjectionError,
    > {
        let Some(region) = node
            .transfer
            .as_ref()
            .and_then(|transfer| transfer.process.as_ref())
            .and_then(|process| process.control.as_ref())
        else {
            return Ok(None);
        };
        let frame = node
            .frame
            .process
            .as_ref()
            .ok_or(ProjectionError::SourceMismatch)?;
        let relation = self
            .directories
            .get(&region.relation)
            .copied()
            .ok_or(ProjectionError::SourceMismatch)?;
        if frame.epoch != relation.epoch
            || frame.source != relation.source
            || frame.location != region.entry
            || relation.program.is_some_and(|id| {
                self.programs
                    .get(&id)
                    .is_none_or(|program| !frame.source.matches_program(program))
            })
        {
            return Err(ProjectionError::SourceMismatch);
        }
        Ok(Some((region, relation)))
    }

    pub(super) fn source_edges(&mut self) -> Result<(), ProjectionError> {
        if self.overview {
            return Ok(());
        }
        for node in self.members.clone() {
            let Some((region, relation)) = self.directory(node)? else {
                continue;
            };
            // Copy the small directory handles before mutating the display.
            let through = region.through.clone();
            let relation = *self.directories.get(&relation.id).unwrap();
            for location in through {
                let Some(from) = self.point(node, location) else {
                    continue;
                };
                match location {
                    ProcessLocation::Block(index) => {
                        if let Some(block) = relation.blocks.get(index) {
                            for successor in block.successors.iter().flatten() {
                                if let Some(to) = self.point(node, successor.to) {
                                    edge(
                                        &mut self.projection.edges,
                                        from,
                                        to,
                                        successor.kind,
                                        EdgeLevel::Source,
                                        FlowOrigin::Source {
                                            node: node.id,
                                            relation: relation.id,
                                            from: location,
                                            to: successor.to,
                                        },
                                    );
                                }
                            }
                        }
                        // A source continuation is not a return delivery.
                    }
                    ProcessLocation::JumpTargets => self.jump_targets(node, from, relation),
                    ProcessLocation::Any | ProcessLocation::End => {}
                }
            }
        }
        Ok(())
    }

    pub(super) fn unexpanded_jumps(&mut self) {
        if self.overview {
            return;
        }
        for card in &mut self.projection.cards {
            if !matches!(
                card.kind,
                CardKind::Source {
                    location: ProcessLocation::JumpTargets,
                    ..
                }
            ) || card.target_set.is_some()
            {
                continue;
            }
            let frame = card
                .origins
                .first()
                .and_then(|origin| {
                    self.members
                        .iter()
                        .find(|node| node.id == origin.target.origin.node)
                })
                .and_then(|node| node.frame.process.as_ref());
            let relation = frame.and_then(|frame| {
                self.directories.values().copied().find(|relation| {
                    relation.source == frame.source && relation.epoch == frame.epoch
                })
            });
            let Some(relation) = relation else { continue };
            let Some(program) = relation.program else {
                continue;
            };
            let entries = relation
                .jump_targets
                .iter()
                .map(|&target| TargetEntry {
                    destination: Destination::Source {
                        program,
                        location: ProcessLocation::Block(target),
                    },
                    via: FlowOrigin::DecodedJump {
                        relation: relation.id,
                        target,
                    },
                })
                .collect();
            card.target_set = Some(self.projection.target_sets.len());
            self.projection.target_sets.push(TargetSet {
                kind: TargetSetKind::JumpTargets {
                    relation: relation.id,
                },
                entries,
            });
        }
    }

    fn jump_targets(
        &mut self,
        node: &RegionalNode,
        from: usize,
        relation: &ProcessControlRelation,
    ) {
        let mut entries = Vec::new();
        for &target in &relation.jump_targets {
            let location = ProcessLocation::Block(target);
            let destination = if let Some(id) = self.point(node, location) {
                let origin = self.projection.cards[id]
                    .origins
                    .iter()
                    .find(|origin| origin.target.origin.node == node.id)
                    .expect("point map retains its originating receipt");
                Destination::Control(origin.target)
            } else if let Some(program) = relation.program {
                Destination::Source { program, location }
            } else {
                continue;
            };
            entries.push(TargetEntry {
                destination,
                via: FlowOrigin::Jump {
                    node: node.id,
                    relation: relation.id,
                    target,
                },
            });
        }
        if let Some(index) = self.projection.cards[from].target_set {
            self.projection.target_sets[index].entries.extend(entries);
        } else {
            let index = self.projection.target_sets.len();
            self.projection.target_sets.push(TargetSet {
                kind: TargetSetKind::JumpTargets {
                    relation: relation.id,
                },
                entries,
            });
            self.projection.cards[from].target_set = Some(index);
        }
        // The complete set is inspectable without laying out a Cartesian fanout.
        // Only actual retained local edges enter the source graph.
    }
}
