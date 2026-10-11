//! The "Save query" form shown in a dialog: a saved query by default, with
//! a way out to a `.sql` file.

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    form::{Field, Form},
    input::{Input, InputEvent, InputState},
    radio::RadioGroup,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::rc::Rc;

/// Called with (name, available-for-every-engine) when the user confirms;
/// returns true when the dialog may close.
pub type SubmitHandler = Rc<dyn Fn(String, bool, &mut Window, &mut App) -> bool>;

/// Called when the user would rather save to a `.sql` file (the dialog has
/// already closed).
pub type FileHandler = Rc<dyn Fn(&mut Window, &mut App)>;

pub struct SaveQueryForm {
    name: Entity<InputState>,
    /// Display name of the connection's engine ("PostgreSQL").
    engine: SharedString,
    global: bool,
    to_file: FileHandler,
    _subscription: Subscription,
}

impl SaveQueryForm {
    pub fn new(
        initial_name: String,
        engine: String,
        submit: SubmitHandler,
        to_file: FileHandler,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Name, e.g. Monthly revenue")
                .default_value(initial_name)
        });
        let subscription = cx.subscribe_in(
            &name,
            window,
            move |this: &mut Self, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. })
                    && this.submit(&submit, window, cx)
                {
                    window.close_dialog(cx);
                }
            },
        );
        Self {
            name,
            engine: engine.into(),
            global: false,
            to_file,
            _subscription: subscription,
        }
    }

    pub fn name(&self, cx: &App) -> String {
        self.name.read(cx).value().to_string()
    }

    pub fn is_global(&self) -> bool {
        self.global
    }

    /// Focus the name field.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.name.update(cx, |input, cx| input.focus(window, cx));
    }

    fn submit(
        &mut self,
        submit: &SubmitHandler,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let name = self.name(cx);
        submit(name, self.global, window, cx)
    }
}

impl Render for SaveQueryForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scopes: [SharedString; 2] = [
            format!("{} connections only", self.engine).into(),
            "Every connection".into(),
        ];
        Form::new()
            .child(Field::new().label("Name").child(Input::new(&self.name)))
            .child(
                Field::new().label("Available for").child(
                    RadioGroup::new("save-query-scope")
                        .children(scopes)
                        .selected_index(Some(usize::from(self.global)))
                        .on_change(cx.listener(|this, index: &usize, _, cx| {
                            this.global = *index == 1;
                            cx.notify();
                        })),
                ),
            )
            .child(
                Field::new().child(
                    Button::new("save-to-file")
                        .ghost()
                        .small()
                        .icon(IconName::FileDown)
                        .label("Save to a .sql file instead…")
                        .on_click(cx.listener(|this, _, window, cx| {
                            window.close_dialog(cx);
                            let to_file = this.to_file.clone();
                            to_file(window, cx);
                        })),
                ),
            )
    }
}
