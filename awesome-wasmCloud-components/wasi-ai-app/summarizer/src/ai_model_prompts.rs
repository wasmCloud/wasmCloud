use serde::{Deserialize, Serialize};
use tracing::info;

#[derive(Serialize, Deserialize)]
struct UsrPromptContext {
    transcription: String,
}

#[derive(Serialize, Deserialize)]
struct SysPromptContext {
    // for Qwen Model we can disable think by adding /no_think in sys prompt
    sys_prompt_ctx: String,
}

static USER_PROMPT_TEMPLATE_PATH: &str = "data/user-prompt.txt";
static SYS_PROMPT_TEMPLATE_PATH: &str = "data/system-prompt.txt";

pub fn configure_prompts(transcription: String, sys_prompt_ctx: String) -> Result<String, Box<dyn std::error::Error>> {

    info!("Configuring prompts.");

    info!("*****************************************");
    
    info!(transcription=transcription, "user_prompt_ctx");
    info!(think=sys_prompt_ctx, "sys_prompt_ctx");

    info!(USER_PROMPT_TEMPLATE_PATH, "USER_PROMPT_TEMPLATE_PATH");
    
    let mut tt = tinytemplate::TinyTemplate::new();

    let user_prompt_template = std::fs::read_to_string(USER_PROMPT_TEMPLATE_PATH)?;

    info!(user_prompt_template=user_prompt_template, "USER_PROMPT_TEMPLATE");
    
    tt.add_template("usr", user_prompt_template.as_str())?;
    
    let upt_ctx = UsrPromptContext {
        transcription: transcription.into(),
    };
    
    let user_prompt = tt.render("usr", &upt_ctx)?;

    info!(SYS_PROMPT_TEMPLATE_PATH, "SYS_PROMPT_TEMPLATE_PATH");
    
    let sys_prompt_template = std::fs::read_to_string(SYS_PROMPT_TEMPLATE_PATH)?;

    info!(sys_prompt_template=sys_prompt_template, "SYS_PROMPT_TEMPLATE");
    
    tt.add_template("sys", sys_prompt_template.as_str())?;
    
    let ctx = SysPromptContext { 
        sys_prompt_ctx : sys_prompt_ctx.into(),
    };

    let system_prompt = tt.render("sys", &ctx)?;
    
    info!(system_prompt=system_prompt, "AI assistant system prompt.");
    
    let prompt_str = format!(
        "{0}<|im_start|>user\n{1}<|im_end|>\n<|im_start|>assistant\n",
        system_prompt, 
        user_prompt
    );

    info!("*****************************************");
    
    Ok(prompt_str)
}
