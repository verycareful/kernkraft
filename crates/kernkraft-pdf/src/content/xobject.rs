// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! External objects and marked content in the interpreter: ISO
//! 32000-1:2008, 8.8 (external objects, `Do`), 8.10 (form XObjects,
//! Table 95), 14.6 (marked content, Table 352) and 8.11.3 (making
//! graphic content optional, the `OC` entry).

use crate::content::device::{
    FillRule, FormInfo, ImageSource, MarkedContent, OperatorSpan, Path, PathSegment, StreamId,
};
use crate::content::interpreter::{Env, Interpreter};
use crate::content::matrix::Matrix;
use crate::content::operation::{operand_number, Operation};
use crate::content::state::SoftMask;
use crate::diagnostic::DiagnosticKind;
use crate::doc::Content;
use crate::error::{Limit, PdfError};
use crate::syntax::limits::MAX_FORM_DEPTH;
use crate::syntax::{Dictionary, Name, Object, ObjectId, Value};

impl Interpreter {
    /// `Do`: paints the image, runs the form.
    pub(super) fn do_xobject(
        &mut self,
        operation: &Operation,
        span: OperatorSpan,
        env: &mut Env<'_>,
    ) -> Result<(), PdfError> {
        let Some(name) = self.name_operand(operation) else {
            return Ok(());
        };
        let Some(entry) = self.lookup(b"XObject", &name)? else {
            self.missing(b"XObject", &name, operation);
            return Ok(());
        };
        let Value::Reference(id) = entry.value() else {
            // An XObject is always a stream, so an indirect object.
            self.invalid(operation);
            return Ok(());
        };
        let id = *id;
        let Some(loaded) = self.doc.object(id)? else {
            self.missing(b"XObject", &name, operation);
            return Ok(());
        };
        let Content::Stream(stream) = &loaded.content else {
            self.invalid(operation);
            return Ok(());
        };
        let subtype = match stream.dictionary.get(b"Subtype").map(Object::value) {
            Some(Value::Name(subtype)) => Some(subtype.clone()),
            _ => None,
        };
        match subtype.as_ref().map(Name::as_bytes) {
            Some(b"Image") => {
                let hidden = self.hides(&stream.dictionary)?;
                if hidden {
                    self.hidden_forms += 1;
                }
                let image = ImageSource::XObject { name, id };
                let ctx = self.context(span);
                env.device.image(&ctx, &image);
                if hidden {
                    self.hidden_forms -= 1;
                }
                Ok(())
            }
            Some(b"Form") => self.invoke_form(id, Some(name), span, env),
            _ => {
                self.diagnostics.push(
                    DiagnosticKind::XObjectSubtypeUnsupported {
                        subtype: subtype.unwrap_or_else(|| Name::new(Vec::new())),
                    },
                    operation.span.start,
                );
                Ok(())
            }
        }
    }

    /// Whether the `OC` entry of `dictionary` puts it in a group that is off
    /// (8.11.3.2): for an annotation, an XObject, or any other dictionary
    /// that has the entry.
    pub fn hides_content(&mut self, dictionary: &Dictionary) -> Result<bool, PdfError> {
        self.hides(dictionary)
    }

    /// Whether the `OC` entry of `dictionary` puts it in a group that is off.
    fn hides(&mut self, dictionary: &Dictionary) -> Result<bool, PdfError> {
        match dictionary.get(b"OC") {
            Some(oc) => Ok(!self
                .optional
                .is_visible(&self.doc, oc, &mut self.diagnostics)?),
            None => Ok(false),
        }
    }

    /// Runs the form XObject `id` (8.10.2): the state is saved, the form's
    /// `Matrix` concatenated to the CTM, the `BBox` clipped to, and the
    /// form's content run with its own resources; the state is then
    /// restored.
    pub(super) fn invoke_form(
        &mut self,
        id: ObjectId,
        name: Option<Name>,
        span: OperatorSpan,
        env: &mut Env<'_>,
    ) -> Result<(), PdfError> {
        if self.forms.contains(&id) {
            self.diagnostics.set_object(Some(id));
            self.diagnostics
                .push(DiagnosticKind::FormRecursion, span.range.start);
            self.diagnostics.set_object(None);
            return Ok(());
        }
        if self.forms.len() >= MAX_FORM_DEPTH {
            return Err(PdfError::LimitExceeded {
                offset: span.range.start,
                limit: Limit::FormNesting,
            });
        }
        let loaded = self.doc.required(id)?;
        let Content::Stream(stream) = &loaded.content else {
            return Err(PdfError::WrongKind { object: id });
        };
        let dictionary = &stream.dictionary;
        let matrix = self.form_matrix(dictionary)?;
        let bbox = self.form_bbox(dictionary)?;
        let group = match dictionary.get(b"Group") {
            Some(group) => self.doc.resolve(group)?.dictionary().cloned(),
            None => None,
        };
        let info = FormInfo {
            id,
            name,
            bbox,
            matrix,
            group,
        };
        let hidden = self.hides(dictionary)?;
        let resources = match dictionary.get(b"Resources") {
            Some(resources) => self
                .doc
                .resolve(resources)?
                .dictionary()
                .cloned()
                .unwrap_or_default(),
            // A form with no resources of its own uses the page's, or the
            // enclosing form's (7.8.3, deprecated in 2.0 but still met).
            None => self.resources.last().cloned().unwrap_or_default(),
        };
        let content = self.doc.load_content_stream(
            StreamId::Object(id),
            self.options.max_stream_bytes,
            &mut self.diagnostics,
        )?;

        self.push_state(span.range.start)?;
        let ctx = self.context(span);
        env.device.save(&ctx);
        self.state.ctm = matrix.then(self.state.ctm);
        if hidden {
            self.hidden_forms += 1;
        }
        self.forms.push(id);
        self.resources.push(resources);
        let ctx = self.context(span);
        env.device.begin_form(&ctx, &info);
        if info.group.is_some() {
            // The group is composed as one object with the alpha, blend
            // mode and soft mask it was invoked with, so what it paints
            // starts with none of them (11.6.6).
            self.state.fill_alpha = 1.0;
            self.state.stroke_alpha = 1.0;
            self.state.blend_mode = Name::new(b"Normal".to_vec());
            self.state.soft_mask = SoftMask::None;
            self.state.alpha_is_shape = false;
        }
        if let Some([llx, lly, urx, ury]) = bbox {
            let clip = Path {
                segments: vec![
                    PathSegment::MoveTo(llx, lly),
                    PathSegment::LineTo(urx, lly),
                    PathSegment::LineTo(urx, ury),
                    PathSegment::LineTo(llx, ury),
                    PathSegment::Close,
                ],
            };
            let ctx = self.context(span);
            env.device.clip(&ctx, &clip, FillRule::NonZeroWinding);
        }
        self.diagnostics.set_object(Some(id));
        let result = self.run_content(StreamId::Object(id), content.data, env);
        self.diagnostics.set_object(None);
        let ctx = self.context(span);
        env.device.end_form(&ctx, &info);
        self.resources.pop();
        self.forms.pop();
        if hidden {
            self.hidden_forms -= 1;
        }
        self.restore_state(None, env);
        result
    }

    fn form_matrix(&self, dictionary: &Dictionary) -> Result<Matrix, PdfError> {
        let Some(entry) = dictionary.get(b"Matrix") else {
            return Ok(Matrix::IDENTITY);
        };
        let resolved = self.doc.resolve(entry)?;
        let Some(items) = resolved.array() else {
            return Ok(Matrix::IDENTITY);
        };
        let mut values = [0.0; 6];
        if items.len() != values.len() {
            return Ok(Matrix::IDENTITY);
        }
        for (slot, item) in values.iter_mut().zip(items) {
            match operand_number(self.doc.resolve(item)?.object()) {
                Some(number) => *slot = number,
                None => return Ok(Matrix::IDENTITY),
            }
        }
        Ok(Matrix::from_array(values))
    }

    fn form_bbox(&self, dictionary: &Dictionary) -> Result<Option<[f64; 4]>, PdfError> {
        let Some(entry) = dictionary.get(b"BBox") else {
            return Ok(None);
        };
        let resolved = self.doc.resolve(entry)?;
        let Some(items) = resolved.array() else {
            return Ok(None);
        };
        let mut values = Vec::with_capacity(items.len());
        for item in items {
            match operand_number(self.doc.resolve(item)?.object()) {
                Some(number) => values.push(number),
                None => return Ok(None),
            }
        }
        let [x1, y1, x2, y2] = values.as_slice() else {
            return Ok(None);
        };
        // The corners may be given in either order (7.9.5).
        Ok(Some([x1.min(*x2), y1.min(*y2), x1.max(*x2), y1.max(*y2)]))
    }

    /// `MP`, `DP`, `BMC`, `BDC` and `EMC` (14.6.2, Table 352).
    pub(super) fn marked_content(
        &mut self,
        operation: &Operation,
        span: OperatorSpan,
        env: &mut Env<'_>,
    ) -> Result<(), PdfError> {
        let operator = operation.operator.as_slice();
        if operator == b"EMC" {
            if !operation.operands.is_empty() {
                self.invalid(operation);
                return Ok(());
            }
            if self.marked.len() <= self.base_marked {
                self.diagnostics.push(
                    DiagnosticKind::MarkedContentUnbalanced,
                    operation.span.start,
                );
                return Ok(());
            }
            let Some(closed) = self.marked.pop() else {
                return Ok(());
            };
            let ctx = self.context(span);
            env.device.end_marked(&ctx, &closed);
            return Ok(());
        }
        let with_properties = matches!(operator, b"DP" | b"BDC");
        let (tag, properties) = match (operation.operands.as_slice(), with_properties) {
            ([tag], false) => (tag, None),
            ([tag, properties], true) => (tag, Some(properties)),
            _ => {
                self.invalid(operation);
                return Ok(());
            }
        };
        let Value::Name(tag) = tag.value() else {
            self.invalid(operation);
            return Ok(());
        };
        let tag = tag.clone();
        let mut sequence = MarkedContent {
            tag: tag.clone(),
            properties: None,
            properties_name: None,
            properties_id: None,
            hidden: false,
        };
        // The property list: a dictionary written in place, or the name of
        // an entry of the `Properties` resources.
        let mut list_object: Option<Object> = None;
        if let Some(properties) = properties {
            match properties.value() {
                Value::Dictionary(dictionary) => {
                    sequence.properties = Some(dictionary.clone());
                    list_object = Some(properties.clone());
                }
                Value::Name(name) => match self.lookup(b"Properties", name)? {
                    Some(entry) => {
                        if let Value::Reference(id) = entry.value() {
                            sequence.properties_id = Some(*id);
                        }
                        sequence.properties = self.doc.resolve(&entry)?.dictionary().cloned();
                        sequence.properties_name = Some(name.clone());
                        list_object = Some(entry);
                    }
                    None => {
                        self.missing(b"Properties", name, operation);
                        return Ok(());
                    }
                },
                _ => {
                    self.invalid(operation);
                    return Ok(());
                }
            }
        }
        if tag.as_bytes() == b"OC" && operator == b"BDC" {
            if let Some(object) = &list_object {
                sequence.hidden =
                    !self
                        .optional
                        .is_visible(&self.doc, object, &mut self.diagnostics)?;
            }
        }
        if with_properties && operator == b"DP" {
            let ctx = self.context(span);
            env.device
                .marked_point(&ctx, &tag, sequence.properties.as_ref());
            return Ok(());
        }
        if operator == b"MP" {
            let ctx = self.context(span);
            env.device.marked_point(&ctx, &tag, None);
            return Ok(());
        }
        self.marked.push(sequence);
        let ctx = self.context(span);
        env.device.begin_marked(&ctx);
        Ok(())
    }
}
