#!/usr/bin/env python3
import re

with open('src/app.rs', 'r') as f:
    content = f.read()

# Replace the one_shot prompt format section
old_one_shot = '''Response Format:\n\
1. Outcome: one sentence stating what was completed.\n\
2. Changes made: concise bullet list.\n\
3. Verification: commands or checks run and their results.\n\
4. Blockers: only if something could not be completed.\n\n\
Definition of done:\n\
- The objective is completed.\n\
- Results are verified where possible.\n\
- The final response is concise and specific.'''

new_one_shot = '''Completion Summary Format (required):\n\
When finished, return a short but specific summary using this exact format:\n\n\
Done: [one sentence describing what was completed.]\n\n\
How: [one sentence describing the method or mechanism.]\n\n\
Count: [specific numbers for what was found, processed, changed, skipped, failed, or created.]\n\
Include whichever counts are relevant: items processed, created, updated, skipped, failed, files read/written, records changed, API calls made.\n\
Example: \"Processed 42 records: 39 updated, 2 skipped, 1 failed.\"\n\n\
Important: [only the most relevant caveats, output locations, assumptions, warnings, or next steps. If no issues, use: No issues detected.]\n\n\
Definition of done:\n\
- The objective is completed.\n\
- Results are verified where possible.\n\
- The final response follows the completion summary format above.'''

if old_one_shot in content:
    content = content.replace(old_one_shot, new_one_shot)
    print('Updated one_shot_task_prompt')
else:
    print('ERROR: Could not find one_shot pattern')

# Replace the sharded prompt format section
old_sharded = '''Response Format:\n\
1. Delegate summary: one sentence of what your slice accomplished.\n\
2. Changes made: concise bullet list.\n\
3. Verification: checks run and outcomes.\n\
4. Handoff: what another delegate or the coordinator should do next, if anything.'''

new_sharded = '''Completion Summary Format (required):\n\
When finished, return a short but specific summary using this exact format:\n\n\
Done: [one sentence describing what was completed in your slice.]\n\n\
How: [one sentence describing the method or mechanism used.]\n\n\
Count: [specific numbers for what was found, processed, changed, skipped, failed, or created.]\n\
Include whichever counts are relevant: items processed, created, updated, skipped, failed, files read/written, records changed, API calls made.\n\
Example: \"Processed 42 records: 39 updated, 2 skipped, 1 failed.\"\n\n\
Important: [only the most relevant caveats, output locations, assumptions, warnings, or next steps. Include handoff notes here if another delegate or the coordinator should do something next. If no issues, use: No issues detected.]'''

if old_sharded in content:
    content = content.replace(old_sharded, new_sharded)
    print('Updated sharded_task_prompt')
else:
    print('ERROR: Could not find sharded pattern')

with open('src/app.rs', 'w') as f:
    f.write(content)

print('Done!')
